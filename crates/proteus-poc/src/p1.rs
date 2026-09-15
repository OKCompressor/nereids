//! Proteus P1 explicit binary checkpoint and append-only journal.
//!
//! PRT0/PJR0 are deliberately encoded here instead of through a general
//! serializer. Logical hashes omit physical paths and offsets.

use crate::{
    resume_snapshot_in_memory_unserialized, DictionaryBaseLayerV0, DictionaryDeltaLayerV0,
    DictionaryEntryV0, DuDictionaryLayersV0, IncrementalCandidateV0, ModelTokenizerIdentityV0,
    NativeTokenPieceV0, ProteusSnapshotV0, SnapshotProvenanceEventV0, SNAPSHOT_SCHEMA,
    SNAPSHOT_VERSION,
};
use anyhow::{anyhow, bail, Context, Result};
use nereids::direct_id::{construct_du_aware_repair_candidate, DirectIdSnapshotV2};
use nereids::{bytes_to_hex, LoadedStream, TokenSpan};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(unix)]
use std::os::fd::AsRawFd;

pub type Hash32 = [u8; 32];

pub const PRT_VERSION: u16 = 1;
pub const PJR_VERSION: u16 = 1;
pub const PRT_HEADER_LEN: usize = 88;
pub const SECTION_DESCRIPTOR_LEN: usize = 56;
pub const PJR_FOOTER_LEN: usize = 128;
const TAIL_BLOCK_LEN: u64 = 64 * 1024;

const PRT_MAGIC: &[u8; 4] = b"PRT0";
const META_MAGIC: &[u8; 4] = b"PMT0";
const FOOTER_MAGIC: &[u8; 4] = b"PJR0";
const DESCRIPTOR_OPTIONAL: u16 = 1;

const CK_IDENTITY: u16 = 1;
const CK_TRANSCRIPT: u16 = 2;
const CK_NATIVE_IDS: u16 = 3;
const CK_NATIVE_SPANS: u16 = 4;
const CK_DU_DICTIONARY: u16 = 5;
const CK_DU_STREAM: u16 = 6;
const CK_PROVENANCE: u16 = 7;
const CK_STATE: u16 = 8;
/// Reserved optional checkpoint section. P1 defines no trust policy for it.
pub const CK_SIGNATURE: u16 = 0x8001;

pub const TRANSCRIPT_DELTA: u16 = 1;
pub const DU_DICT_DELTA: u16 = 2;
pub const DU_STREAM_DELTA: u16 = 3;
pub const NATIVE_ID_DELTA: u16 = 4;
pub const NATIVE_SPAN_DELTA: u16 = 5;

const CHECKPOINT_REQUIRED: [u16; 8] = [
    CK_IDENTITY,
    CK_TRANSCRIPT,
    CK_NATIVE_IDS,
    CK_NATIVE_SPANS,
    CK_DU_DICTIONARY,
    CK_DU_STREAM,
    CK_PROVENANCE,
    CK_STATE,
];
const JOURNAL_REQUIRED: [u16; 5] = [
    TRANSCRIPT_DELTA,
    DU_DICT_DELTA,
    DU_STREAM_DELTA,
    NATIVE_ID_DELTA,
    NATIVE_SPAN_DELTA,
];

fn hash_domain(domain: &[u8], parts: &[&[u8]]) -> Hash32 {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn hash_bytes(bytes: &[u8]) -> Hash32 {
    Sha256::digest(bytes).into()
}

pub fn hash_hex(hash: &Hash32) -> String {
    hex::encode(hash)
}

pub fn parse_hash_hex(value: &str) -> Result<Hash32> {
    let bytes = hex::decode(value).context("decode 32-byte hash")?;
    bytes
        .try_into()
        .map_err(|_| anyhow!("hash must contain exactly 32 bytes"))
}

fn usize_from_u64(value: u64, what: &str) -> Result<usize> {
    usize::try_from(value).with_context(|| format!("{what} does not fit this platform"))
}

fn u64_from_usize(value: usize, what: &str) -> Result<u64> {
    u64::try_from(value).with_context(|| format!("{what} exceeds u64"))
}

fn checked_range(offset: u64, len: u64, limit: u64, what: &str) -> Result<(usize, usize)> {
    let end = offset
        .checked_add(len)
        .with_context(|| format!("{what} range overflow"))?;
    if end > limit {
        bail!("{what} range {offset}..{end} exceeds limit {limit}");
    }
    Ok((usize_from_u64(offset, what)?, usize_from_u64(end, what)?))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SectionDescriptor {
    kind: u16,
    flags: u16,
    offset: u64,
    len: u64,
    hash: Hash32,
}

impl SectionDescriptor {
    fn encode(&self, out: &mut Vec<u8>) {
        put_u16(out, self.kind);
        put_u16(out, self.flags);
        put_u32(out, 0);
        put_u64(out, self.offset);
        put_u64(out, self.len);
        out.extend_from_slice(&self.hash);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let kind = reader.u16()?;
        let flags = reader.u16()?;
        if flags & !DESCRIPTOR_OPTIONAL != 0 {
            bail!("section kind {kind} has unsupported flags {flags:#x}");
        }
        if reader.u32()? != 0 {
            bail!("section descriptor reserved field is nonzero");
        }
        Ok(Self {
            kind,
            flags,
            offset: reader.u64()?,
            len: reader.u64()?,
            hash: reader.hash()?,
        })
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(len)
            .context("binary field position overflow")?;
        let value = self
            .bytes
            .get(self.position..end)
            .context("truncated binary field")?;
        self.position = end;
        Ok(value)
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| anyhow!("invalid u16 field width"))?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| anyhow!("invalid u32 field width"))?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| anyhow!("invalid u64 field width"))?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn u128(&mut self) -> Result<u128> {
        let bytes: [u8; 16] = self
            .take(16)?
            .try_into()
            .map_err(|_| anyhow!("invalid u128 field width"))?;
        Ok(u128::from_le_bytes(bytes))
    }

    fn hash(&mut self) -> Result<Hash32> {
        self.take(32)?
            .try_into()
            .map_err(|_| anyhow!("invalid SHA-256 field width"))
    }

    fn bytes_u64(&mut self) -> Result<Vec<u8>> {
        let len = usize_from_u64(self.u64()?, "byte string length")?;
        Ok(self.take(len)?.to_vec())
    }

    fn string(&mut self) -> Result<String> {
        String::from_utf8(self.bytes_u64()?).context("binary UTF-8 field is invalid")
    }

    fn finish(self, what: &str) -> Result<()> {
        if self.position != self.bytes.len() {
            bail!(
                "{what} has {} trailing bytes",
                self.bytes.len() - self.position
            );
        }
        Ok(())
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u128(out: &mut Vec<u8>, value: u128) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    put_u64(out, u64_from_usize(bytes.len(), "byte string length")?);
    out.extend_from_slice(bytes);
    Ok(())
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<()> {
    put_bytes(out, value.as_bytes())
}

fn count(reader: &mut Reader<'_>, minimum_item_len: usize, what: &str) -> Result<usize> {
    let value = usize_from_u64(reader.u64()?, what)?;
    if minimum_item_len > 0 && value > reader.remaining() / minimum_item_len {
        bail!("impossible {what} {value} for remaining bytes");
    }
    Ok(value)
}

fn encode_identity(identity: &ModelTokenizerIdentityV0, lineage: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    put_string(&mut out, lineage)?;
    put_string(&mut out, &identity.model_id)?;
    put_string(&mut out, &identity.model_provenance)?;
    put_string(&mut out, &identity.tokenizer_id)?;
    put_string(&mut out, &identity.tokenizer_provenance)?;
    Ok(out)
}

fn decode_identity(bytes: &[u8]) -> Result<(String, ModelTokenizerIdentityV0)> {
    let mut reader = Reader::new(bytes);
    let lineage = reader.string()?;
    let identity = ModelTokenizerIdentityV0 {
        model_id: reader.string()?,
        model_provenance: reader.string()?,
        tokenizer_id: reader.string()?,
        tokenizer_provenance: reader.string()?,
    };
    reader.finish("checkpoint identity section")?;
    Ok((lineage, identity))
}

fn encode_native_ids(ids: &[u32]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    put_u64(&mut out, u64_from_usize(ids.len(), "native ID count")?);
    for id in ids {
        put_u32(&mut out, *id);
    }
    Ok(out)
}

fn decode_native_ids(bytes: &[u8]) -> Result<Vec<u32>> {
    let mut reader = Reader::new(bytes);
    let len = count(&mut reader, 4, "native ID count")?;
    let mut ids = Vec::with_capacity(len);
    for _ in 0..len {
        ids.push(reader.u32()?);
    }
    reader.finish("native ID section")?;
    Ok(ids)
}

fn encode_spans(spans: &[TokenSpan]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    put_u64(&mut out, u64_from_usize(spans.len(), "native span count")?);
    for span in spans {
        let bytes = nereids::hex_to_bytes(&span.bytes_hex).context("encode native span bytes")?;
        put_u64(&mut out, span.id);
        put_u64(
            &mut out,
            u64_from_usize(span.byte_start, "native span start")?,
        );
        put_u64(&mut out, u64_from_usize(span.byte_end, "native span end")?);
        put_bytes(&mut out, &bytes)?;
    }
    Ok(out)
}

fn decode_spans(bytes: &[u8]) -> Result<Vec<TokenSpan>> {
    let mut reader = Reader::new(bytes);
    let len = count(&mut reader, 32, "native span count")?;
    let mut spans = Vec::with_capacity(len);
    for _ in 0..len {
        let id = reader.u64()?;
        if id > u64::from(u32::MAX) {
            bail!("native span ID exceeds canonical u32 native ID range");
        }
        let byte_start = usize_from_u64(reader.u64()?, "native span start")?;
        let byte_end = usize_from_u64(reader.u64()?, "native span end")?;
        if byte_end <= byte_start {
            bail!("native span is empty or reversed");
        }
        let exact = reader.bytes_u64()?;
        if exact.len()
            != byte_end
                .checked_sub(byte_start)
                .context("native span underflow")?
        {
            bail!("native span exact byte length disagrees with its range");
        }
        spans.push(TokenSpan {
            id,
            byte_start,
            byte_end,
            bytes_hex: bytes_to_hex(&exact),
        });
    }
    reader.finish("native span section")?;
    Ok(spans)
}

fn encode_entry(out: &mut Vec<u8>, entry: &DictionaryEntryV0) -> Result<()> {
    if entry.lexeme_bytes.is_empty() {
        bail!("cannot encode empty DU dictionary lexeme");
    }
    put_u64(out, entry.id);
    put_bytes(out, &entry.lexeme_bytes)
}

fn decode_entry(reader: &mut Reader<'_>) -> Result<DictionaryEntryV0> {
    let id = reader.u64()?;
    let lexeme_bytes = reader.bytes_u64()?;
    if lexeme_bytes.is_empty() {
        bail!("decoded empty DU dictionary lexeme");
    }
    Ok(DictionaryEntryV0 { id, lexeme_bytes })
}

fn encode_dictionary(dictionary: &DuDictionaryLayersV0) -> Result<Vec<u8>> {
    dictionary.validate()?;
    let mut out = Vec::new();
    put_u64(&mut out, dictionary.next_id);
    put_u64(
        &mut out,
        u64_from_usize(dictionary.base.entries.len(), "base dictionary count")?,
    );
    for entry in &dictionary.base.entries {
        encode_entry(&mut out, entry)?;
    }
    put_u64(
        &mut out,
        u64_from_usize(dictionary.deltas.len(), "dictionary layer count")?,
    );
    for layer in &dictionary.deltas {
        put_u64(&mut out, layer.append_sequence);
        put_u64(
            &mut out,
            u64_from_usize(layer.entries.len(), "dictionary layer entry count")?,
        );
        for entry in &layer.entries {
            encode_entry(&mut out, entry)?;
        }
    }
    Ok(out)
}

fn decode_dictionary(bytes: &[u8], lineage: String) -> Result<DuDictionaryLayersV0> {
    let mut reader = Reader::new(bytes);
    let next_id = reader.u64()?;
    let base_len = count(&mut reader, 16, "base dictionary count")?;
    let mut base = Vec::with_capacity(base_len);
    for _ in 0..base_len {
        base.push(decode_entry(&mut reader)?);
    }
    let delta_len = count(&mut reader, 16, "dictionary layer count")?;
    let mut deltas = Vec::with_capacity(delta_len);
    for _ in 0..delta_len {
        let append_sequence = reader.u64()?;
        let entry_len = count(&mut reader, 16, "dictionary layer entry count")?;
        let mut entries = Vec::with_capacity(entry_len);
        for _ in 0..entry_len {
            entries.push(decode_entry(&mut reader)?);
        }
        deltas.push(DictionaryDeltaLayerV0 {
            append_sequence,
            entries,
        });
    }
    reader.finish("DU dictionary section")?;
    let dictionary = DuDictionaryLayersV0 {
        lineage,
        next_id,
        base: DictionaryBaseLayerV0 { entries: base },
        deltas,
    };
    dictionary.validate()?;
    Ok(dictionary)
}

fn encode_stream(stream: &LoadedStream) -> Result<Vec<u8>> {
    stream.validate_structure()?;
    let mut out = Vec::new();
    put_u64(
        &mut out,
        u64_from_usize(stream.ids.len(), "DU stream count")?,
    );
    for (id, bytes) in stream.ids.iter().zip(&stream.parts) {
        put_u64(&mut out, *id);
        put_bytes(&mut out, bytes)?;
    }
    Ok(out)
}

fn decode_stream(bytes: &[u8]) -> Result<LoadedStream> {
    let mut reader = Reader::new(bytes);
    let len = count(&mut reader, 16, "DU stream count")?;
    let mut ids = Vec::with_capacity(len);
    let mut parts = Vec::with_capacity(len);
    let mut bounds = Vec::with_capacity(len.checked_add(1).context("DU bounds count overflow")?);
    bounds.push(0);
    let mut end = 0usize;
    for _ in 0..len {
        ids.push(reader.u64()?);
        let part = reader.bytes_u64()?;
        if part.is_empty() {
            bail!("DU stream contains an empty lexeme");
        }
        end = end
            .checked_add(part.len())
            .context("DU stream length overflow")?;
        bounds.push(end);
        parts.push(part);
    }
    reader.finish("DU stream section")?;
    let stream = LoadedStream { ids, parts, bounds };
    stream.validate_structure()?;
    Ok(stream)
}

fn encode_event(out: &mut Vec<u8>, event: &SnapshotProvenanceEventV0) -> Result<()> {
    put_u64(out, event.append_sequence);
    put_u128(out, event.unix_time_ns);
    put_string(out, &event.actor)?;
    put_string(out, &event.source)
}

fn decode_event(reader: &mut Reader<'_>) -> Result<SnapshotProvenanceEventV0> {
    Ok(SnapshotProvenanceEventV0 {
        append_sequence: reader.u64()?,
        unix_time_ns: reader.u128()?,
        actor: reader.string()?,
        source: reader.string()?,
    })
}

fn encode_provenance(snapshot: &ProteusSnapshotV0) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    encode_event(&mut out, &snapshot.creation_provenance)?;
    put_u64(
        &mut out,
        u64_from_usize(snapshot.update_provenance.len(), "update provenance count")?,
    );
    for event in &snapshot.update_provenance {
        encode_event(&mut out, event)?;
    }
    Ok(out)
}

fn decode_provenance(
    bytes: &[u8],
) -> Result<(SnapshotProvenanceEventV0, Vec<SnapshotProvenanceEventV0>)> {
    let mut reader = Reader::new(bytes);
    let creation = decode_event(&mut reader)?;
    let len = count(&mut reader, 40, "update provenance count")?;
    let mut updates = Vec::with_capacity(len);
    for _ in 0..len {
        updates.push(decode_event(&mut reader)?);
    }
    reader.finish("checkpoint provenance section")?;
    Ok((creation, updates))
}

fn checkpoint_root(sequence: u64, descriptors: &[SectionDescriptor]) -> Result<Hash32> {
    let mut logical = Vec::new();
    logical.extend_from_slice(b"PROTEUS:PRT0:ROOT:V1\0");
    put_u16(&mut logical, PRT_VERSION);
    put_u16(&mut logical, 0);
    put_u64(&mut logical, sequence);
    put_u64(
        &mut logical,
        u64_from_usize(descriptors.len(), "checkpoint logical section count")?,
    );
    let mut logical_descriptors = descriptors.iter().collect::<Vec<_>>();
    logical_descriptors.sort_by_key(|descriptor| descriptor.kind);
    for descriptor in logical_descriptors {
        put_u16(&mut logical, descriptor.kind);
        put_u16(&mut logical, descriptor.flags);
        put_u64(&mut logical, descriptor.len);
        logical.extend_from_slice(&descriptor.hash);
    }
    Ok(hash_bytes(&logical))
}

#[derive(Clone, Debug)]
pub struct P1Checkpoint {
    pub sequence: u64,
    pub root: Hash32,
    pub snapshot: ProteusSnapshotV0,
    pub encoded_len: u64,
}

pub fn encode_checkpoint(snapshot: &ProteusSnapshotV0) -> Result<(Vec<u8>, Hash32)> {
    snapshot.validate()?;
    let transcript = snapshot.direct_id.du_state.reconstruct()?;
    let mut state = Vec::new();
    put_u64(
        &mut state,
        u64_from_usize(snapshot.transcript_len, "transcript length")?,
    );
    state.extend_from_slice(&parse_hash_hex(&snapshot.transcript_sha256)?);
    put_u64(&mut state, snapshot.dictionary_layers.next_id);

    let sections = vec![
        (
            CK_IDENTITY,
            0,
            encode_identity(&snapshot.model_tokenizer, &snapshot.dictionary_lineage)?,
        ),
        (CK_TRANSCRIPT, 0, transcript),
        (
            CK_NATIVE_IDS,
            0,
            encode_native_ids(&snapshot.direct_id.native_ids)?,
        ),
        (
            CK_NATIVE_SPANS,
            0,
            encode_spans(&snapshot.direct_id.native_spans)?,
        ),
        (
            CK_DU_DICTIONARY,
            0,
            encode_dictionary(&snapshot.dictionary_layers)?,
        ),
        (
            CK_DU_STREAM,
            0,
            encode_stream(&snapshot.direct_id.du_state)?,
        ),
        (CK_PROVENANCE, 0, encode_provenance(snapshot)?),
        (CK_STATE, 0, state),
    ];
    let directory_len = sections
        .len()
        .checked_mul(SECTION_DESCRIPTOR_LEN)
        .context("checkpoint directory length overflow")?;
    let mut offset = PRT_HEADER_LEN
        .checked_add(directory_len)
        .context("checkpoint payload offset overflow")?;
    let mut descriptors = Vec::with_capacity(sections.len());
    for (kind, flags, bytes) in &sections {
        descriptors.push(SectionDescriptor {
            kind: *kind,
            flags: *flags,
            offset: u64_from_usize(offset, "checkpoint section offset")?,
            len: u64_from_usize(bytes.len(), "checkpoint section length")?,
            hash: hash_bytes(bytes),
        });
        offset = offset
            .checked_add(bytes.len())
            .context("checkpoint length overflow")?;
    }
    let root = checkpoint_root(snapshot.append_sequence, &descriptors)?;
    let mut out = Vec::with_capacity(offset);
    out.extend_from_slice(PRT_MAGIC);
    put_u16(&mut out, PRT_VERSION);
    put_u16(&mut out, 0);
    put_u64(&mut out, PRT_HEADER_LEN as u64);
    put_u64(&mut out, snapshot.append_sequence);
    put_u64(
        &mut out,
        u64_from_usize(sections.len(), "checkpoint section count")?,
    );
    put_u64(&mut out, PRT_HEADER_LEN as u64);
    put_u64(
        &mut out,
        u64_from_usize(directory_len, "checkpoint directory length")?,
    );
    out.extend_from_slice(&root);
    put_u64(&mut out, 0);
    debug_assert_eq!(out.len(), PRT_HEADER_LEN);
    for descriptor in &descriptors {
        descriptor.encode(&mut out);
    }
    for (_, _, bytes) in sections {
        out.extend_from_slice(&bytes);
    }
    Ok((out, root))
}

fn validate_section_set(
    descriptors: &[SectionDescriptor],
    required: &[u16],
    known_optional: &[u16],
) -> Result<()> {
    let mut seen = HashSet::new();
    for descriptor in descriptors {
        if !seen.insert(descriptor.kind) {
            bail!("duplicate section kind {}", descriptor.kind);
        }
        if required.contains(&descriptor.kind) && descriptor.flags != 0 {
            bail!(
                "required section kind {} is marked optional",
                descriptor.kind
            );
        }
        if known_optional.contains(&descriptor.kind) && descriptor.flags & DESCRIPTOR_OPTIONAL == 0
        {
            bail!(
                "optional section kind {} lacks the optional flag",
                descriptor.kind
            );
        }
        if !required.contains(&descriptor.kind) && !known_optional.contains(&descriptor.kind) {
            if descriptor.flags & DESCRIPTOR_OPTIONAL == 0 {
                bail!("unknown required section kind {}", descriptor.kind);
            }
        }
    }
    for kind in required {
        if !seen.contains(kind) {
            bail!("missing required section kind {kind}");
        }
    }
    Ok(())
}

fn validate_ranges(
    descriptors: &[SectionDescriptor],
    minimum: u64,
    maximum: u64,
    require_packed: bool,
) -> Result<()> {
    let mut ranges = descriptors
        .iter()
        .map(|descriptor| {
            let end = descriptor
                .offset
                .checked_add(descriptor.len)
                .context("section range overflow")?;
            if descriptor.offset < minimum || end > maximum {
                bail!(
                    "section kind {} range {}..{} is outside {}..{}",
                    descriptor.kind,
                    descriptor.offset,
                    end,
                    minimum,
                    maximum
                );
            }
            Ok((descriptor.offset, end, descriptor.kind))
        })
        .collect::<Result<Vec<_>>>()?;
    ranges.sort_unstable();
    let mut cursor = minimum;
    for (start, end, kind) in ranges {
        if start < cursor {
            bail!("canonical section kind {kind} overlaps an earlier section");
        }
        if require_packed && start != cursor {
            bail!("canonical section kind {kind} is not contiguously packed");
        }
        cursor = end;
    }
    if require_packed && cursor != maximum {
        bail!("canonical sections do not fill their declared payload range");
    }
    Ok(())
}

fn section_bytes<'a>(
    bytes: &'a [u8],
    descriptors: &[SectionDescriptor],
    kind: u16,
) -> Result<&'a [u8]> {
    let descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.kind == kind)
        .with_context(|| format!("missing section kind {kind}"))?;
    let (start, end) = checked_range(
        descriptor.offset,
        descriptor.len,
        u64_from_usize(bytes.len(), "checkpoint bytes length")?,
        "section",
    )?;
    Ok(&bytes[start..end])
}

pub fn decode_checkpoint(bytes: &[u8]) -> Result<P1Checkpoint> {
    if bytes.len() < PRT_HEADER_LEN {
        bail!("truncated PRT0 header");
    }
    let mut reader = Reader::new(&bytes[..PRT_HEADER_LEN]);
    if reader.take(4)? != PRT_MAGIC {
        bail!("invalid PRT0 magic");
    }
    if reader.u16()? != PRT_VERSION {
        bail!("unsupported PRT0 version");
    }
    if reader.u16()? != 0 {
        bail!("unsupported PRT0 flags");
    }
    if reader.u64()? != PRT_HEADER_LEN as u64 {
        bail!("invalid PRT0 header length");
    }
    let sequence = reader.u64()?;
    let section_count = usize_from_u64(reader.u64()?, "checkpoint section count")?;
    let directory_offset = reader.u64()?;
    let directory_len = reader.u64()?;
    let stored_root = reader.hash()?;
    if reader.u64()? != 0 {
        bail!("PRT0 reserved header field is nonzero");
    }
    reader.finish("PRT0 header")?;
    if directory_offset != PRT_HEADER_LEN as u64 {
        bail!("PRT0 directory must immediately follow its header");
    }
    let expected_directory_len = section_count
        .checked_mul(SECTION_DESCRIPTOR_LEN)
        .context("checkpoint directory size overflow")?;
    if directory_len != u64_from_usize(expected_directory_len, "checkpoint directory length")? {
        bail!("PRT0 directory length/count mismatch");
    }
    let (_, directory_end) = checked_range(
        directory_offset,
        directory_len,
        u64_from_usize(bytes.len(), "checkpoint bytes length")?,
        "checkpoint directory",
    )?;
    let mut directory_reader = Reader::new(&bytes[PRT_HEADER_LEN..directory_end]);
    let mut descriptors = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        descriptors.push(SectionDescriptor::decode(&mut directory_reader)?);
    }
    directory_reader.finish("PRT0 directory")?;
    validate_section_set(&descriptors, &CHECKPOINT_REQUIRED, &[CK_SIGNATURE])?;
    validate_ranges(
        &descriptors,
        u64_from_usize(directory_end, "checkpoint payload start")?,
        u64_from_usize(bytes.len(), "checkpoint file length")?,
        true,
    )?;
    for descriptor in &descriptors {
        let section = section_bytes(bytes, &descriptors, descriptor.kind)?;
        if hash_bytes(section) != descriptor.hash {
            bail!("checkpoint section kind {} hash mismatch", descriptor.kind);
        }
    }
    let computed_root = checkpoint_root(sequence, &descriptors)?;
    if stored_root != computed_root {
        bail!("checkpoint root mismatch");
    }

    let (lineage, model_tokenizer) =
        decode_identity(section_bytes(bytes, &descriptors, CK_IDENTITY)?)?;
    let transcript = section_bytes(bytes, &descriptors, CK_TRANSCRIPT)?.to_vec();
    let native_ids = decode_native_ids(section_bytes(bytes, &descriptors, CK_NATIVE_IDS)?)?;
    let native_spans = decode_spans(section_bytes(bytes, &descriptors, CK_NATIVE_SPANS)?)?;
    let dictionary_layers = decode_dictionary(
        section_bytes(bytes, &descriptors, CK_DU_DICTIONARY)?,
        lineage.clone(),
    )?;
    let du_state = decode_stream(section_bytes(bytes, &descriptors, CK_DU_STREAM)?)?;
    let (creation_provenance, update_provenance) =
        decode_provenance(section_bytes(bytes, &descriptors, CK_PROVENANCE)?)?;
    let mut state_reader = Reader::new(section_bytes(bytes, &descriptors, CK_STATE)?);
    let transcript_len = usize_from_u64(state_reader.u64()?, "checkpoint transcript length")?;
    let transcript_hash = state_reader.hash()?;
    let next_id = state_reader.u64()?;
    state_reader.finish("checkpoint state section")?;
    if transcript.len() != transcript_len || hash_bytes(&transcript) != transcript_hash {
        bail!("checkpoint transcript identity mismatch");
    }
    if dictionary_layers.next_id != next_id {
        bail!("checkpoint next canonical DU ID mismatch");
    }
    let direct_id = DirectIdSnapshotV2::new(&transcript, native_ids, native_spans, du_state)
        .map_err(anyhow::Error::new)?;
    let snapshot = ProteusSnapshotV0 {
        schema: SNAPSHOT_SCHEMA.to_owned(),
        version: SNAPSHOT_VERSION,
        append_sequence: sequence,
        dictionary_lineage: lineage,
        model_tokenizer,
        transcript_len,
        transcript_sha256: hash_hex(&transcript_hash),
        direct_id,
        dictionary_layers,
        creation_provenance,
        update_provenance,
    };
    snapshot.validate()?;
    Ok(P1Checkpoint {
        sequence,
        root: computed_root,
        snapshot,
        encoded_len: u64_from_usize(bytes.len(), "checkpoint encoded length")?,
    })
}

pub fn create_checkpoint(path: &Path, snapshot: &ProteusSnapshotV0) -> Result<P1Checkpoint> {
    let (bytes, _) = encode_checkpoint(snapshot)?;
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .with_context(|| format!("create checkpoint directory {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create immutable checkpoint {}", path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("write checkpoint {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync checkpoint {}", path.display()))?;
    decode_checkpoint(&bytes)
}

pub fn open_checkpoint(path: &Path) -> Result<P1Checkpoint> {
    let bytes = fs::read(path).with_context(|| format!("read checkpoint {}", path.display()))?;
    decode_checkpoint(&bytes).with_context(|| format!("validate checkpoint {}", path.display()))
}

#[derive(Clone, Debug)]
struct JournalMeta {
    sequence: u64,
    parent: Hash32,
    base_checkpoint_root: Hash32,
    result_transcript_len: u64,
    result_transcript_hash: Hash32,
    next_canonical_du_id: u64,
    provenance: SnapshotProvenanceEventV0,
    descriptors: Vec<SectionDescriptor>,
    record_root: Hash32,
}

fn record_root(meta: &JournalMeta) -> Result<Hash32> {
    let mut logical = Vec::new();
    logical.extend_from_slice(b"PROTEUS:PJR0:RECORD:V1\0");
    put_u16(&mut logical, PJR_VERSION);
    put_u16(&mut logical, 0);
    put_u64(&mut logical, meta.sequence);
    logical.extend_from_slice(&meta.parent);
    logical.extend_from_slice(&meta.base_checkpoint_root);
    put_u64(&mut logical, meta.result_transcript_len);
    logical.extend_from_slice(&meta.result_transcript_hash);
    put_u64(&mut logical, meta.next_canonical_du_id);
    encode_event(&mut logical, &meta.provenance)?;
    put_u64(
        &mut logical,
        u64_from_usize(meta.descriptors.len(), "journal section count")?,
    );
    let mut logical_descriptors = meta.descriptors.iter().collect::<Vec<_>>();
    logical_descriptors.sort_by_key(|descriptor| descriptor.kind);
    for descriptor in logical_descriptors {
        put_u16(&mut logical, descriptor.kind);
        put_u16(&mut logical, descriptor.flags);
        put_u64(&mut logical, descriptor.len);
        logical.extend_from_slice(&descriptor.hash);
    }
    Ok(hash_bytes(&logical))
}

fn encode_meta(meta: &JournalMeta) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(META_MAGIC);
    put_u16(&mut out, PJR_VERSION);
    put_u16(&mut out, 0);
    put_u64(&mut out, meta.sequence);
    out.extend_from_slice(&meta.parent);
    out.extend_from_slice(&meta.base_checkpoint_root);
    put_u64(&mut out, meta.result_transcript_len);
    out.extend_from_slice(&meta.result_transcript_hash);
    put_u64(&mut out, meta.next_canonical_du_id);
    encode_event(&mut out, &meta.provenance)?;
    put_u64(
        &mut out,
        u64_from_usize(meta.descriptors.len(), "journal section count")?,
    );
    for descriptor in &meta.descriptors {
        descriptor.encode(&mut out);
    }
    out.extend_from_slice(&meta.record_root);
    Ok(out)
}

fn decode_meta(bytes: &[u8]) -> Result<JournalMeta> {
    let mut reader = Reader::new(bytes);
    if reader.take(4)? != META_MAGIC {
        bail!("invalid PJR0 META magic");
    }
    if reader.u16()? != PJR_VERSION {
        bail!("unsupported PJR0 META version");
    }
    if reader.u16()? != 0 {
        bail!("unsupported PJR0 META flags");
    }
    let sequence = reader.u64()?;
    let parent = reader.hash()?;
    let base_checkpoint_root = reader.hash()?;
    let result_transcript_len = reader.u64()?;
    let result_transcript_hash = reader.hash()?;
    let next_canonical_du_id = reader.u64()?;
    let provenance = decode_event(&mut reader)?;
    let section_count = count(&mut reader, SECTION_DESCRIPTOR_LEN, "journal section count")?;
    let mut descriptors = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        descriptors.push(SectionDescriptor::decode(&mut reader)?);
    }
    let stored_root = reader.hash()?;
    reader.finish("PJR0 META")?;
    validate_section_set(&descriptors, &JOURNAL_REQUIRED, &[])?;
    let meta = JournalMeta {
        sequence,
        parent,
        base_checkpoint_root,
        result_transcript_len,
        result_transcript_hash,
        next_canonical_du_id,
        provenance,
        descriptors,
        record_root: stored_root,
    };
    let computed = record_root(&meta)?;
    if stored_root != computed {
        bail!("PJR0 record root mismatch");
    }
    Ok(meta)
}

#[derive(Clone, Debug)]
struct Footer {
    meta_offset: u64,
    meta_len: u64,
    meta_hash: Hash32,
    record_root: Hash32,
}

fn encode_footer(footer: &Footer) -> Vec<u8> {
    let mut out = Vec::with_capacity(PJR_FOOTER_LEN);
    out.extend_from_slice(FOOTER_MAGIC);
    put_u16(&mut out, PJR_VERSION);
    put_u16(&mut out, 0);
    put_u64(&mut out, PJR_FOOTER_LEN as u64);
    put_u64(&mut out, footer.meta_offset);
    put_u64(&mut out, footer.meta_len);
    out.extend_from_slice(&footer.meta_hash);
    out.extend_from_slice(&footer.record_root);
    let checksum = hash_domain(b"PROTEUS:PJR0:FOOTER:V1\0", &[&out]);
    out.extend_from_slice(&checksum);
    debug_assert_eq!(out.len(), PJR_FOOTER_LEN);
    out
}

fn decode_footer(bytes: &[u8]) -> Result<Footer> {
    if bytes.len() != PJR_FOOTER_LEN {
        bail!("PJR0 footer must be exactly {PJR_FOOTER_LEN} bytes");
    }
    let expected_checksum = hash_domain(
        b"PROTEUS:PJR0:FOOTER:V1\0",
        &[&bytes[..PJR_FOOTER_LEN - 32]],
    );
    if &bytes[PJR_FOOTER_LEN - 32..] != expected_checksum.as_slice() {
        bail!("PJR0 footer checksum mismatch");
    }
    let mut reader = Reader::new(bytes);
    if reader.take(4)? != FOOTER_MAGIC {
        bail!("invalid PJR0 footer magic");
    }
    if reader.u16()? != PJR_VERSION {
        bail!("unsupported PJR0 footer version");
    }
    if reader.u16()? != 0 {
        bail!("unsupported PJR0 footer flags");
    }
    if reader.u64()? != PJR_FOOTER_LEN as u64 {
        bail!("invalid PJR0 fixed footer length");
    }
    let footer = Footer {
        meta_offset: reader.u64()?,
        meta_len: reader.u64()?,
        meta_hash: reader.hash()?,
        record_root: reader.hash()?,
    };
    let _checksum = reader.hash()?;
    reader.finish("PJR0 footer")?;
    Ok(footer)
}

#[derive(Clone, Debug)]
struct LocatedRecord {
    footer_offset: u64,
    footer_end: u64,
    record_start: u64,
    meta: JournalMeta,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TailDiscovery {
    pub committed_sequence: Option<u64>,
    pub committed_head: Option<String>,
    pub footer_offset: Option<u64>,
    pub committed_end: u64,
    pub trailing_uncommitted_bytes: u64,
    pub bytes_read: u64,
}

fn read_exact_at(
    file: &mut File,
    offset: u64,
    len: usize,
    bytes_read: &mut u64,
) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; len];
    file.read_exact(&mut bytes)?;
    *bytes_read = bytes_read
        .checked_add(u64_from_usize(len, "read byte count")?)
        .context("read byte counter overflow")?;
    Ok(bytes)
}

fn validate_footer_candidate(
    file: &mut File,
    footer_offset: u64,
    file_len: u64,
    footer_bytes: &[u8],
    bytes_read: &mut u64,
) -> Result<LocatedRecord> {
    let footer = decode_footer(footer_bytes)?;
    let footer_end = footer_offset
        .checked_add(PJR_FOOTER_LEN as u64)
        .context("footer end overflow")?;
    if footer_end > file_len {
        bail!("footer extends past EOF");
    }
    let meta_end = footer
        .meta_offset
        .checked_add(footer.meta_len)
        .context("META range overflow")?;
    if meta_end != footer_offset {
        bail!("PJR0 META must immediately precede its footer");
    }
    let meta_len = usize_from_u64(footer.meta_len, "PJR0 META length")?;
    let meta_bytes = read_exact_at(file, footer.meta_offset, meta_len, bytes_read)?;
    if hash_bytes(&meta_bytes) != footer.meta_hash {
        bail!("PJR0 META hash mismatch");
    }
    let meta = decode_meta(&meta_bytes)?;
    if meta.record_root != footer.record_root {
        bail!("PJR0 footer and META record roots differ");
    }
    let record_start = meta
        .descriptors
        .iter()
        .map(|descriptor| descriptor.offset)
        .min()
        .context("journal record has no payload descriptors")?;
    validate_ranges(&meta.descriptors, record_start, footer.meta_offset, true)?;
    Ok(LocatedRecord {
        footer_offset,
        footer_end,
        record_start,
        meta,
    })
}

/// Discover the newest complete valid footer by searching backward from
/// `upper_bound` in bounded blocks. Candidate magic is accepted only after
/// footer checksum, META range/hash, section ranges, and record root validate.
fn discover_before(
    file: &mut File,
    upper_bound: u64,
    bytes_read: &mut u64,
) -> Result<Option<LocatedRecord>> {
    if upper_bound < PJR_FOOTER_LEN as u64 {
        return Ok(None);
    }
    let mut block_end = upper_bound;
    while block_end > 0 {
        let overlap = (PJR_FOOTER_LEN - 1) as u64;
        let nominal_start = block_end.saturating_sub(TAIL_BLOCK_LEN);
        let start = nominal_start.saturating_sub(overlap);
        let len = usize_from_u64(block_end - start, "tail discovery block length")?;
        let block = read_exact_at(file, start, len, bytes_read)?;
        let mut position = block.len().saturating_sub(4);
        loop {
            if block.get(position..position + 4) == Some(FOOTER_MAGIC.as_slice()) {
                let absolute = start
                    .checked_add(u64_from_usize(position, "footer candidate position")?)
                    .context("footer candidate offset overflow")?;
                let end = absolute.saturating_add(PJR_FOOTER_LEN as u64);
                if end <= upper_bound {
                    let footer_bytes = if position + PJR_FOOTER_LEN <= block.len() {
                        block[position..position + PJR_FOOTER_LEN].to_vec()
                    } else {
                        read_exact_at(file, absolute, PJR_FOOTER_LEN, bytes_read)?
                    };
                    if let Ok(record) = validate_footer_candidate(
                        file,
                        absolute,
                        upper_bound,
                        &footer_bytes,
                        bytes_read,
                    ) {
                        return Ok(Some(record));
                    }
                }
            }
            if position == 0 {
                break;
            }
            position -= 1;
        }
        if nominal_start == 0 {
            break;
        }
        block_end = nominal_start.checked_add(overlap).unwrap_or(nominal_start);
    }
    Ok(None)
}

pub fn discover_journal_tail(path: &Path) -> Result<TailDiscovery> {
    if !path.exists() {
        return Ok(TailDiscovery::default());
    }
    let mut file = File::open(path).with_context(|| format!("open journal {}", path.display()))?;
    let file_len = file.metadata()?.len();
    let mut bytes_read = 0;
    let record = discover_before(&mut file, file_len, &mut bytes_read)?;
    Ok(match record {
        Some(record) => TailDiscovery {
            committed_sequence: Some(record.meta.sequence),
            committed_head: Some(hash_hex(&record.meta.record_root)),
            footer_offset: Some(record.footer_offset),
            committed_end: record.footer_end,
            trailing_uncommitted_bytes: file_len - record.footer_end,
            bytes_read,
        },
        None => TailDiscovery {
            trailing_uncommitted_bytes: file_len,
            bytes_read,
            ..TailDiscovery::default()
        },
    })
}

fn collect_lineage(path: &Path) -> Result<(Vec<LocatedRecord>, u64, u64)> {
    if !path.exists() {
        return Ok((Vec::new(), 0, 0));
    }
    let mut file = File::open(path).with_context(|| format!("open journal {}", path.display()))?;
    let file_len = file.metadata()?.len();
    let mut bytes_read = 0;
    let Some(mut current) = discover_before(&mut file, file_len, &mut bytes_read)? else {
        // With no committed footer, every journal byte is an uncommitted first
        // append. The immutable checkpoint remains the accepted state.
        return Ok((Vec::new(), bytes_read, 0));
    };
    let committed_end = current.footer_end;
    let mut reverse = vec![current.clone()];
    while current.record_start > 0 {
        let Some(previous) = discover_before(&mut file, current.record_start, &mut bytes_read)?
        else {
            // Bytes before the first committed record may be an interrupted
            // first append. Causal parent/sequence validation below decides
            // whether the discovered record can actually start this lineage.
            break;
        };
        current = previous;
        reverse.push(current.clone());
    }
    reverse.reverse();
    Ok((reverse, bytes_read, committed_end))
}

fn read_payload(
    file: &mut File,
    descriptor: &SectionDescriptor,
    bytes_read: &mut u64,
) -> Result<Vec<u8>> {
    let len = usize_from_u64(descriptor.len, "journal section length")?;
    let bytes = read_exact_at(file, descriptor.offset, len, bytes_read)?;
    if hash_bytes(&bytes) != descriptor.hash {
        bail!("journal section kind {} hash mismatch", descriptor.kind);
    }
    Ok(bytes)
}

fn descriptor<'a>(meta: &'a JournalMeta, kind: u16) -> Result<&'a SectionDescriptor> {
    meta.descriptors
        .iter()
        .find(|descriptor| descriptor.kind == kind)
        .with_context(|| format!("missing journal section kind {kind}"))
}

fn encode_dictionary_delta(layer: &DictionaryDeltaLayerV0) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    put_u64(&mut out, layer.append_sequence);
    put_u64(
        &mut out,
        u64_from_usize(layer.entries.len(), "dictionary delta entry count")?,
    );
    for entry in &layer.entries {
        encode_entry(&mut out, entry)?;
    }
    Ok(out)
}

fn decode_dictionary_delta(bytes: &[u8]) -> Result<DictionaryDeltaLayerV0> {
    let mut reader = Reader::new(bytes);
    let append_sequence = reader.u64()?;
    let len = count(&mut reader, 16, "dictionary delta entry count")?;
    let mut entries = Vec::with_capacity(len);
    for _ in 0..len {
        entries.push(decode_entry(&mut reader)?);
    }
    reader.finish("DU dictionary delta")?;
    Ok(DictionaryDeltaLayerV0 {
        append_sequence,
        entries,
    })
}

fn encode_native_id_delta(kept: usize, suffix: &[u32]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    put_u64(&mut out, u64_from_usize(kept, "kept native ID count")?);
    put_u64(
        &mut out,
        u64_from_usize(suffix.len(), "native ID suffix count")?,
    );
    for id in suffix {
        put_u32(&mut out, *id);
    }
    Ok(out)
}

fn decode_native_id_delta(bytes: &[u8]) -> Result<(usize, Vec<u32>)> {
    let mut reader = Reader::new(bytes);
    let kept = usize_from_u64(reader.u64()?, "kept native ID count")?;
    let len = count(&mut reader, 4, "native ID suffix count")?;
    let mut suffix = Vec::with_capacity(len);
    for _ in 0..len {
        suffix.push(reader.u32()?);
    }
    reader.finish("native ID delta")?;
    Ok((kept, suffix))
}

fn encode_native_span_delta(
    kept: usize,
    repair_start: usize,
    suffix: &[TokenSpan],
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    put_u64(&mut out, u64_from_usize(kept, "kept native span count")?);
    put_u64(
        &mut out,
        u64_from_usize(repair_start, "native repair start")?,
    );
    out.extend_from_slice(&encode_spans(suffix)?);
    Ok(out)
}

fn decode_native_span_delta(bytes: &[u8]) -> Result<(usize, usize, Vec<TokenSpan>)> {
    let mut reader = Reader::new(bytes);
    let kept = usize_from_u64(reader.u64()?, "kept native span count")?;
    let repair_start = usize_from_u64(reader.u64()?, "native repair start")?;
    let remaining = reader.remaining();
    let spans = decode_spans(reader.take(remaining)?)?;
    Ok((kept, repair_start, spans))
}

fn stream_suffix(stream: &LoadedStream, start: usize) -> Result<LoadedStream> {
    stream.validate_structure()?;
    if start > stream.ids.len() {
        bail!("DU stream suffix starts beyond stream token count");
    }
    let parts = stream.parts[start..].to_vec();
    let boundary_count = parts
        .len()
        .checked_add(1)
        .context("DU suffix bounds overflow")?;
    let mut bounds = Vec::with_capacity(boundary_count);
    bounds.push(0);
    let mut end = 0usize;
    for part in &parts {
        end = end
            .checked_add(part.len())
            .context("DU suffix length overflow")?;
        bounds.push(end);
    }
    let suffix = LoadedStream {
        ids: stream.ids[start..].to_vec(),
        parts,
        bounds,
    };
    suffix.validate_structure()?;
    Ok(suffix)
}

fn apply_record(
    previous: &ProteusSnapshotV0,
    checkpoint_root: &Hash32,
    expected_parent: &Hash32,
    record: &LocatedRecord,
    file: &mut File,
    bytes_read: &mut u64,
) -> Result<ProteusSnapshotV0> {
    let meta = &record.meta;
    let expected_sequence = previous
        .append_sequence
        .checked_add(1)
        .context("journal sequence overflow")?;
    if meta.sequence != expected_sequence {
        bail!(
            "journal record sequence {} does not follow state sequence {}",
            meta.sequence,
            previous.append_sequence
        );
    }
    if &meta.parent != expected_parent {
        bail!("journal record parent does not match accepted canonical head");
    }
    if &meta.base_checkpoint_root != checkpoint_root {
        bail!("journal record base checkpoint root mismatch");
    }
    if meta.provenance.append_sequence != meta.sequence {
        bail!("journal provenance sequence mismatch");
    }

    let transcript_delta = read_payload(file, descriptor(meta, TRANSCRIPT_DELTA)?, bytes_read)?;
    let dictionary_delta = decode_dictionary_delta(&read_payload(
        file,
        descriptor(meta, DU_DICT_DELTA)?,
        bytes_read,
    )?)?;
    if dictionary_delta.append_sequence != meta.sequence {
        bail!("DU dictionary delta sequence mismatch");
    }
    let stream_delta = decode_stream(&read_payload(
        file,
        descriptor(meta, DU_STREAM_DELTA)?,
        bytes_read,
    )?)?;
    if stream_delta.reconstruct()? != transcript_delta {
        bail!("DU stream delta does not reconstruct transcript delta exactly");
    }
    let (kept_ids, native_id_suffix) = decode_native_id_delta(&read_payload(
        file,
        descriptor(meta, NATIVE_ID_DELTA)?,
        bytes_read,
    )?)?;
    let (kept_spans, repair_start, native_span_suffix) = decode_native_span_delta(&read_payload(
        file,
        descriptor(meta, NATIVE_SPAN_DELTA)?,
        bytes_read,
    )?)?;
    if kept_ids != kept_spans {
        bail!("native ID/span delta kept counts differ");
    }
    if kept_ids > previous.direct_id.native_ids.len()
        || kept_spans > previous.direct_id.native_spans.len()
    {
        bail!("native delta keeps more tokens than the previous state");
    }
    let expected_repair_start = previous
        .direct_id
        .native_spans
        .get(kept_spans)
        .map(|span| span.byte_start)
        .unwrap_or(previous.transcript_len);
    if repair_start != expected_repair_start {
        bail!("native repair start disagrees with retained span boundary");
    }
    if native_id_suffix.len() != native_span_suffix.len() {
        bail!("native ID/span suffix counts differ");
    }

    let mut dictionary_layers = previous.dictionary_layers.clone();
    let canonical_delta = dictionary_layers.append_delta(meta.sequence, &stream_delta)?;
    if canonical_delta != stream_delta {
        bail!("persisted DU stream delta contains noncanonical IDs");
    }
    let actual_layer = dictionary_layers
        .deltas
        .last()
        .context("dictionary append created no layer")?;
    if actual_layer != &dictionary_delta {
        bail!("persisted DU dictionary delta violates max+1 first-seen allocation");
    }
    if dictionary_layers.next_id != meta.next_canonical_du_id {
        bail!("journal next canonical DU ID mismatch");
    }
    let combined_du = previous.direct_id.du_state.concatenate(&canonical_delta)?;
    let transcript = combined_du.reconstruct()?;
    if transcript.len() != usize_from_u64(meta.result_transcript_len, "result transcript length")?
        || hash_bytes(&transcript) != meta.result_transcript_hash
    {
        bail!("journal result transcript identity mismatch");
    }

    let mut native_ids = previous.direct_id.native_ids[..kept_ids].to_vec();
    native_ids.extend_from_slice(&native_id_suffix);
    let mut native_spans = previous.direct_id.native_spans[..kept_spans].to_vec();
    native_spans.extend(native_span_suffix);
    let direct_id = DirectIdSnapshotV2::new(&transcript, native_ids, native_spans, combined_du)
        .map_err(anyhow::Error::new)?;
    let mut update_provenance = previous.update_provenance.clone();
    update_provenance.push(meta.provenance.clone());
    let snapshot = ProteusSnapshotV0 {
        schema: SNAPSHOT_SCHEMA.to_owned(),
        version: SNAPSHOT_VERSION,
        append_sequence: meta.sequence,
        dictionary_lineage: previous.dictionary_lineage.clone(),
        model_tokenizer: previous.model_tokenizer.clone(),
        transcript_len: transcript.len(),
        transcript_sha256: hash_hex(&meta.result_transcript_hash),
        direct_id,
        dictionary_layers,
        creation_provenance: previous.creation_provenance.clone(),
        update_provenance,
    };
    snapshot.validate()?;
    Ok(snapshot)
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct P1OpenTimings {
    pub checkpoint_open_wall_ns: u128,
    pub journal_tail_discovery_wall_ns: u128,
    pub bytes_read: u64,
}

#[derive(Clone, Debug)]
pub struct P1Session {
    checkpoint: P1Checkpoint,
    journal_path: PathBuf,
    snapshot: ProteusSnapshotV0,
    head: Hash32,
    committed_end: u64,
    pub open_timings: P1OpenTimings,
}

impl P1Session {
    pub fn open(checkpoint_path: &Path, journal_path: &Path) -> Result<Self> {
        let checkpoint_started = Instant::now();
        let checkpoint_bytes = fs::read(checkpoint_path)
            .with_context(|| format!("read checkpoint {}", checkpoint_path.display()))?;
        let checkpoint = decode_checkpoint(&checkpoint_bytes)
            .with_context(|| format!("validate checkpoint {}", checkpoint_path.display()))?;
        let checkpoint_open_wall_ns = checkpoint_started.elapsed().as_nanos();

        let journal_started = Instant::now();
        let (records, journal_bytes_read, committed_end) = collect_lineage(journal_path)?;
        let mut snapshot = checkpoint.snapshot.clone();
        let mut expected_parent = checkpoint.root;
        let mut replay_bytes_read = 0u64;
        if !records.is_empty() {
            let mut file = File::open(journal_path)?;
            for record in &records {
                snapshot = apply_record(
                    &snapshot,
                    &checkpoint.root,
                    &expected_parent,
                    record,
                    &mut file,
                    &mut replay_bytes_read,
                )?;
                expected_parent = record.meta.record_root;
            }
        }
        let journal_tail_discovery_wall_ns = journal_started.elapsed().as_nanos();
        let bytes_read = u64_from_usize(checkpoint_bytes.len(), "checkpoint read length")?
            .checked_add(journal_bytes_read)
            .and_then(|value| value.checked_add(replay_bytes_read))
            .context("open byte count overflow")?;
        Ok(Self {
            checkpoint,
            journal_path: journal_path.to_owned(),
            snapshot,
            head: expected_parent,
            committed_end,
            open_timings: P1OpenTimings {
                checkpoint_open_wall_ns,
                journal_tail_discovery_wall_ns,
                bytes_read,
            },
        })
    }

    pub fn snapshot(&self) -> &ProteusSnapshotV0 {
        &self.snapshot
    }

    pub fn checkpoint_root(&self) -> Hash32 {
        self.checkpoint.root
    }

    pub fn head(&self) -> Hash32 {
        self.head
    }

    pub fn committed_end(&self) -> u64 {
        self.committed_end
    }
}

/// A loaded P1 state detached from its storage paths for same-process warm work.
///
/// Construction consumes a fully reconstructed `P1Session`. Consequently the
/// warm append API has no checkpoint or journal handle with which it could do
/// file I/O, and every successful append mutates this one snapshot in place.
#[derive(Debug)]
pub struct P1WarmSession {
    checkpoint_root: Hash32,
    snapshot: ProteusSnapshotV0,
    head: Hash32,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct P1WarmBenchSample {
    pub iteration: usize,
    pub append_sequence: u64,
    pub transcript_len: usize,
    pub transcript_sha256: String,
    pub new_dictionary_entries: usize,
    pub incremental_validation_wall_ns: u128,
    pub delta_du_reconstruct_wall_ns: u128,
    pub direct_id_candidate_prep_wall_ns: u128,
    pub native_span_delta_wall_ns: u128,
    pub dictionary_delta_wall_ns: u128,
    pub record_encode_wall_ns: u128,
    pub warm_compute_total_wall_ns: u128,
    pub oracle_full_tokenize_wall_ns: u128,
}

#[derive(Clone, Debug, Serialize)]
pub struct P1WarmBenchReceipt {
    pub schema: &'static str,
    pub iterations: usize,
    pub initial_head: String,
    pub final_head: String,
    pub initial_transcript_len: usize,
    pub final_transcript_len: usize,
    pub initial_load_wall_ns: u128,
    pub all_ids_exact: bool,
    pub oracle_used_only_for_verification: bool,
    pub file_io_inside_warm_loop: bool,
    pub journal_mutated: bool,
    pub samples: Vec<P1WarmBenchSample>,
}

struct PendingWarmVerification {
    sample: P1WarmBenchSample,
    transcript: Vec<u8>,
}

impl P1WarmSession {
    /// Verify the accepted head once and shed every storage path/handle before
    /// the caller enters the warm loop.
    pub fn from_loaded(session: P1Session, expected_head: Hash32) -> Result<Self> {
        if session.head != expected_head {
            bail!(
                "stale expected P1 head {} (accepted head is {})",
                hash_hex(&expected_head),
                hash_hex(&session.head)
            );
        }
        Ok(Self {
            checkpoint_root: session.checkpoint.root,
            snapshot: session.snapshot,
            head: session.head,
        })
    }

    pub fn snapshot(&self) -> &ProteusSnapshotV0 {
        &self.snapshot
    }

    pub fn head(&self) -> Hash32 {
        self.head
    }

    /// Advance the exact loaded state in memory, then use the full tokenizer
    /// only to verify the already-constructed candidate.
    #[allow(clippy::too_many_arguments)]
    pub fn append_verified<TokenizeRepair, Oracle>(
        &mut self,
        iteration: usize,
        delta_bytes: &[u8],
        delta_du: &LoadedStream,
        fixed_repair_depth: usize,
        update_provenance: SnapshotProvenanceEventV0,
        tokenize_repair_window: &mut TokenizeRepair,
        oracle_full_tokenize: &mut Oracle,
    ) -> Result<P1WarmBenchSample>
    where
        TokenizeRepair: FnMut(&[u8]) -> Result<Vec<NativeTokenPieceV0>>,
        Oracle: FnMut(&[u8]) -> Result<Vec<u32>>,
    {
        let pending = self.advance_in_memory(
            iteration,
            delta_bytes,
            delta_du,
            fixed_repair_depth,
            update_provenance,
            tokenize_repair_window,
        )?;
        self.verify_pending(pending, oracle_full_tokenize)
    }

    #[allow(clippy::too_many_arguments)]
    fn advance_in_memory<TokenizeRepair>(
        &mut self,
        iteration: usize,
        delta_bytes: &[u8],
        delta_du: &LoadedStream,
        fixed_repair_depth: usize,
        update_provenance: SnapshotProvenanceEventV0,
        tokenize_repair_window: &mut TokenizeRepair,
    ) -> Result<PendingWarmVerification>
    where
        TokenizeRepair: FnMut(&[u8]) -> Result<Vec<NativeTokenPieceV0>>,
    {
        // This independent interval is frozen before oracle invocation. Stage
        // timers below are diagnostic and are never summed to manufacture it.
        let warm_total_started = Instant::now();

        let validation_started = Instant::now();
        self.snapshot.validate()?;
        let next_sequence = self
            .snapshot
            .append_sequence
            .checked_add(1)
            .context("warm append sequence overflow")?;
        update_provenance.validate_for_sequence(next_sequence)?;
        let mut incremental_validation_wall_ns = validation_started.elapsed().as_nanos();

        let delta_started = Instant::now();
        let reconstructed_delta = delta_du
            .reconstruct()
            .context("reconstruct warm delta DU state")?;
        if reconstructed_delta != delta_bytes {
            bail!("delta DU state does not reconstruct the supplied delta bytes exactly");
        }
        let delta_du_reconstruct_wall_ns = delta_started.elapsed().as_nanos();

        let mut repair_pieces = None::<Vec<NativeTokenPieceV0>>;
        let mut nereids_callback = |window: &[u8]| -> std::result::Result<Vec<u32>, String> {
            let pieces = tokenize_repair_window(window).map_err(|error| error.to_string())?;
            let ids = pieces.iter().map(|piece| piece.id).collect();
            repair_pieces = Some(pieces);
            Ok(ids)
        };
        let candidate_started = Instant::now();
        let (candidate, _) = construct_du_aware_repair_candidate(
            &self.snapshot.direct_id,
            delta_du,
            fixed_repair_depth,
            &mut nereids_callback,
        )
        .map_err(anyhow::Error::new)?;
        let direct_id_candidate_prep_wall_ns = candidate_started.elapsed().as_nanos();
        let repair_pieces =
            repair_pieces.context("native repair tokenizer returned no token pieces")?;

        let span_started = Instant::now();
        let mut repair_window = self
            .snapshot
            .direct_id
            .du_state
            .reconstruct_range(candidate.repair_start_byte, self.snapshot.transcript_len)?;
        repair_window.extend_from_slice(delta_bytes);
        let piece_bytes = repair_pieces
            .iter()
            .flat_map(|piece| piece.bytes.iter().copied())
            .collect::<Vec<_>>();
        if piece_bytes != repair_window {
            bail!("native repair token pieces do not reconstruct the exact repair window");
        }
        let repair_ids = repair_pieces
            .iter()
            .map(|piece| piece.id)
            .collect::<Vec<_>>();
        if &candidate.ids[candidate.kept_snapshot_token_count..] != repair_ids.as_slice() {
            bail!("native repair token pieces disagree with the Direct-ID candidate suffix");
        }
        let expected_len = self
            .snapshot
            .transcript_len
            .checked_add(delta_bytes.len())
            .context("warm transcript byte length overflow")?;
        let mut byte_cursor = candidate.repair_start_byte;
        let mut native_span_suffix = Vec::with_capacity(repair_pieces.len());
        for piece in &repair_pieces {
            if piece.bytes.is_empty() {
                bail!("native repair tokenizer returned an empty token piece");
            }
            let byte_end = byte_cursor
                .checked_add(piece.bytes.len())
                .context("warm native token span byte offset overflow")?;
            native_span_suffix.push(TokenSpan {
                id: u64::from(piece.id),
                byte_start: byte_cursor,
                byte_end,
                bytes_hex: bytes_to_hex(&piece.bytes),
            });
            byte_cursor = byte_end;
        }
        if byte_cursor != expected_len {
            bail!("shifted native spans end at {byte_cursor}, expected {expected_len}");
        }
        self.snapshot
            .direct_id
            .native_ids
            .truncate(candidate.kept_snapshot_token_count);
        self.snapshot
            .direct_id
            .native_ids
            .extend_from_slice(&candidate.ids[candidate.kept_snapshot_token_count..]);
        self.snapshot
            .direct_id
            .native_spans
            .truncate(candidate.kept_snapshot_token_count);
        self.snapshot
            .direct_id
            .native_spans
            .extend(native_span_suffix);
        let native_span_delta_wall_ns = span_started.elapsed().as_nanos();

        let dictionary_started = Instant::now();
        let canonical_delta = self
            .snapshot
            .dictionary_layers
            .append_delta(next_sequence, delta_du)?;
        let new_dictionary_entries = self
            .snapshot
            .dictionary_layers
            .deltas
            .last()
            .context("warm dictionary append produced no delta layer")?
            .entries
            .len();
        let prior_byte_len = self.snapshot.direct_id.du_state.byte_len()?;
        let shifted_bounds = canonical_delta
            .bounds
            .iter()
            .skip(1)
            .map(|boundary| {
                prior_byte_len
                    .checked_add(*boundary)
                    .context("warm DU boundary overflow")
            })
            .collect::<Result<Vec<_>>>()?;
        self.snapshot
            .direct_id
            .du_state
            .ids
            .extend_from_slice(&canonical_delta.ids);
        self.snapshot
            .direct_id
            .du_state
            .parts
            .extend(canonical_delta.parts.iter().cloned());
        self.snapshot
            .direct_id
            .du_state
            .bounds
            .extend(shifted_bounds);
        let transcript = self.snapshot.direct_id.du_state.reconstruct()?;
        let transcript_hash = hash_bytes(&transcript);
        let dictionary_delta_wall_ns = dictionary_started.elapsed().as_nanos();

        self.snapshot.append_sequence = next_sequence;
        self.snapshot.transcript_len = transcript.len();
        self.snapshot.transcript_sha256 = hash_hex(&transcript_hash);
        self.snapshot.direct_id.prefix_len = transcript.len();
        self.snapshot.direct_id.prefix_sha256 = self.snapshot.transcript_sha256.clone();
        self.snapshot
            .update_provenance
            .push(update_provenance.clone());

        let final_validation_started = Instant::now();
        self.snapshot.validate()?;
        incremental_validation_wall_ns = incremental_validation_wall_ns
            .saturating_add(final_validation_started.elapsed().as_nanos());

        let record_started = Instant::now();
        let layer = self
            .snapshot
            .dictionary_layers
            .deltas
            .last()
            .context("warm append produced no dictionary delta layer")?;
        let kept = candidate.kept_snapshot_token_count;
        let payloads = vec![
            (TRANSCRIPT_DELTA, delta_bytes.to_vec()),
            (DU_DICT_DELTA, encode_dictionary_delta(layer)?),
            (DU_STREAM_DELTA, encode_stream(&canonical_delta)?),
            (
                NATIVE_ID_DELTA,
                encode_native_id_delta(kept, &self.snapshot.direct_id.native_ids[kept..])?,
            ),
            (
                NATIVE_SPAN_DELTA,
                encode_native_span_delta(
                    kept,
                    candidate.repair_start_byte,
                    &self.snapshot.direct_id.native_spans[kept..],
                )?,
            ),
        ];
        let mut offset = 0u64;
        let mut descriptors = Vec::with_capacity(payloads.len());
        for (kind, payload) in &payloads {
            descriptors.push(SectionDescriptor {
                kind: *kind,
                flags: 0,
                offset,
                len: u64_from_usize(payload.len(), "warm record payload length")?,
                hash: hash_bytes(payload),
            });
            offset = offset
                .checked_add(u64_from_usize(payload.len(), "warm record payload length")?)
                .context("warm record payload offset overflow")?;
        }
        let mut meta = JournalMeta {
            sequence: next_sequence,
            parent: self.head,
            base_checkpoint_root: self.checkpoint_root,
            result_transcript_len: u64_from_usize(transcript.len(), "warm transcript length")?,
            result_transcript_hash: transcript_hash,
            next_canonical_du_id: self.snapshot.dictionary_layers.next_id,
            provenance: update_provenance,
            descriptors,
            record_root: [0; 32],
        };
        meta.record_root = record_root(&meta)?;
        let _encoded_meta = encode_meta(&meta)?;
        self.head = meta.record_root;
        let record_encode_wall_ns = record_started.elapsed().as_nanos();

        let sample = P1WarmBenchSample {
            iteration,
            append_sequence: next_sequence,
            transcript_len: transcript.len(),
            transcript_sha256: hash_hex(&transcript_hash),
            new_dictionary_entries,
            incremental_validation_wall_ns,
            delta_du_reconstruct_wall_ns,
            direct_id_candidate_prep_wall_ns,
            native_span_delta_wall_ns,
            dictionary_delta_wall_ns,
            record_encode_wall_ns,
            warm_compute_total_wall_ns: warm_total_started.elapsed().as_nanos(),
            oracle_full_tokenize_wall_ns: 0,
        };
        Ok(PendingWarmVerification { sample, transcript })
    }

    fn verify_pending<Oracle>(
        &self,
        mut pending: PendingWarmVerification,
        oracle_full_tokenize: &mut Oracle,
    ) -> Result<P1WarmBenchSample>
    where
        Oracle: FnMut(&[u8]) -> Result<Vec<u32>>,
    {
        let warm_total_before_oracle = pending.sample.warm_compute_total_wall_ns;
        let oracle_started = Instant::now();
        let oracle_ids = oracle_full_tokenize(&pending.transcript)?;
        pending.sample.oracle_full_tokenize_wall_ns = oracle_started.elapsed().as_nanos();
        if pending.sample.warm_compute_total_wall_ns != warm_total_before_oracle {
            bail!("internal timing error: oracle changed warm compute total");
        }
        if self.snapshot.direct_id.native_ids != oracle_ids {
            bail!("warm Direct-ID candidate differs from full-tokenization oracle");
        }
        Ok(pending.sample)
    }
}

fn append_total_excluding_oracle(pre_oracle_ns: u128, post_oracle_ns: u128) -> u128 {
    pre_oracle_ns.saturating_add(post_oracle_ns)
}

#[derive(Debug)]
pub enum P1AppendError {
    StaleHead { expected: Hash32, actual: Hash32 },
    Failed(anyhow::Error),
}

impl fmt::Display for P1AppendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleHead { expected, actual } => write!(
                formatter,
                "stale expected P1 head {} (accepted head is {})",
                hash_hex(expected),
                hash_hex(actual)
            ),
            Self::Failed(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for P1AppendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StaleHead { .. } => None,
            Self::Failed(error) => Some(error.as_ref()),
        }
    }
}

impl From<anyhow::Error> for P1AppendError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(error)
    }
}

impl From<std::io::Error> for P1AppendError {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(error.into())
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct P1AppendTimings {
    pub checkpoint_open_wall_ns: u128,
    pub journal_tail_discovery_wall_ns: u128,
    pub incremental_validation_wall_ns: u128,
    pub delta_du_reconstruct_wall_ns: u128,
    pub direct_id_candidate_prep_wall_ns: u128,
    pub native_span_delta_wall_ns: u128,
    pub dictionary_delta_wall_ns: u128,
    pub journal_encode_wall_ns: u128,
    pub journal_append_wall_ns: u128,
    pub footer_commit_wall_ns: u128,
    pub proteus_p1_append_total_wall_ns: u128,
    pub oracle_full_tokenize_wall_ns: u128,
}

#[derive(Clone, Debug, Serialize)]
pub struct P1AppendReceipt {
    pub schema: &'static str,
    pub checkpoint_root: String,
    pub previous_head: String,
    pub committed_head: String,
    pub append_sequence: u64,
    pub ids_exact: bool,
    pub candidate_constructed_before_oracle: bool,
    pub oracle_used_for_candidate_construction: bool,
    pub oracle_used_for_drop_selection: bool,
    pub oracle_used_for_verification: bool,
    pub transcript_len: usize,
    pub transcript_sha256: String,
    pub dictionary_next_id: u64,
    pub new_dictionary_entries: usize,
    pub bytes_read: u64,
    pub bytes_appended: u64,
    pub historical_canonical_payload_bytes_rewritten: u64,
    pub candidate: IncrementalCandidateV0,
    pub timings: P1AppendTimings,
}

fn current_disk_head(path: &Path, checkpoint_root: Hash32) -> Result<(Hash32, u64, u64)> {
    if !path.exists() {
        return Ok((checkpoint_root, 0, 0));
    }
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut bytes_read = 0;
    let record = discover_before(&mut file, file_len, &mut bytes_read)?;
    Ok(match record {
        Some(record) => (record.meta.record_root, file_len, bytes_read),
        None => (checkpoint_root, file_len, bytes_read),
    })
}

#[cfg(unix)]
struct JournalLock(i32);

#[cfg(unix)]
impl JournalLock {
    fn acquire(file: &File) -> Result<Self> {
        let fd = file.as_raw_fd();
        // SAFETY: `fd` belongs to the live `File` retained by the caller and
        // LOCK_EX does not dereference process memory.
        if unsafe { libc::flock(fd, libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error()).context("lock PJR0 journal");
        }
        Ok(Self(fd))
    }
}

#[cfg(unix)]
impl Drop for JournalLock {
    fn drop(&mut self) {
        // SAFETY: the owning `File` is declared before this guard and therefore
        // remains live until after the guard is dropped.
        let _ = unsafe { libc::flock(self.0, libc::LOCK_UN) };
    }
}

#[cfg(not(unix))]
struct JournalLock;

#[cfg(not(unix))]
impl JournalLock {
    fn acquire(_file: &File) -> Result<Self> {
        bail!("P1 compare-and-set journal locking is currently supported only on Unix")
    }
}

impl P1Session {
    /// Compare-and-set append. The repair tokenizer constructs the Direct-ID
    /// candidate first; only then is the independent full-tokenization oracle
    /// consulted. A stale head or parity failure writes zero bytes.
    #[allow(clippy::too_many_arguments)]
    pub fn append_verified<TokenizeRepair, Oracle>(
        &mut self,
        expected_head: Hash32,
        delta_bytes: &[u8],
        delta_du: &LoadedStream,
        fixed_repair_depth: usize,
        update_provenance: SnapshotProvenanceEventV0,
        tokenize_repair_window: &mut TokenizeRepair,
        oracle_full_tokenize: &mut Oracle,
    ) -> std::result::Result<P1AppendReceipt, P1AppendError>
    where
        TokenizeRepair: FnMut(&[u8]) -> Result<Vec<NativeTokenPieceV0>>,
        Oracle: FnMut(&[u8]) -> Result<Vec<u32>>,
    {
        if let Some(parent) = self
            .journal_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&self.journal_path)
            .with_context(|| format!("open append journal {}", self.journal_path.display()))?;
        let _journal_lock = JournalLock::acquire(&file)?;
        let tail_started = Instant::now();
        let observed_file_len = file.metadata()?.len();
        let mut tail_bytes_read = 0;
        let disk_tip = discover_before(&mut file, observed_file_len, &mut tail_bytes_read)?;
        let disk_head = disk_tip
            .as_ref()
            .map(|record| record.meta.record_root)
            .unwrap_or(self.checkpoint.root);
        let append_tail_ns = tail_started.elapsed().as_nanos();
        if disk_head != expected_head {
            return Err(P1AppendError::StaleHead {
                expected: expected_head,
                actual: disk_head,
            });
        }
        if disk_head != self.head {
            return Err(P1AppendError::StaleHead {
                expected: self.head,
                actual: disk_head,
            });
        }
        let append_total_started = Instant::now();
        let accepted_validation_started = Instant::now();
        if let Some(record) = &disk_tip {
            for descriptor in &record.meta.descriptors {
                let _ = read_payload(&mut file, descriptor, &mut tail_bytes_read)?;
            }
        }
        let accepted_validation_wall_ns = accepted_validation_started.elapsed().as_nanos();
        let initial_file_len = disk_tip
            .as_ref()
            .map(|record| record.footer_end)
            .unwrap_or(0);
        if observed_file_len != initial_file_len {
            // This removes only an uncommitted crash tail after CAS succeeds;
            // no byte at or before the accepted footer is changed.
            file.set_len(initial_file_len)?;
        }

        let prior_snapshot = self.snapshot.clone();
        let artifact = resume_snapshot_in_memory_unserialized(
            &prior_snapshot,
            delta_bytes,
            delta_du,
            fixed_repair_depth,
            update_provenance.clone(),
            tokenize_repair_window,
        )?;
        if !artifact.serialized_snapshot.is_empty() {
            return Err(anyhow!("P1 candidate path unexpectedly serialized JSON state").into());
        }
        let updated = &artifact.updated_snapshot;
        let layer = updated
            .dictionary_layers
            .deltas
            .last()
            .context("P1 append produced no dictionary delta layer")?;
        if layer.append_sequence != updated.append_sequence {
            return Err(anyhow!("P1 append dictionary layer sequence mismatch").into());
        }
        let kept = artifact.candidate.kept_snapshot_token_count;
        if kept > updated.direct_id.native_ids.len() || kept > updated.direct_id.native_spans.len()
        {
            return Err(anyhow!("P1 candidate kept count exceeds updated native state").into());
        }
        let canonical_du_delta = stream_suffix(
            &updated.direct_id.du_state,
            prior_snapshot.direct_id.du_state.ids.len(),
        )?;
        if canonical_du_delta.parts != delta_du.parts
            || canonical_du_delta.bounds != delta_du.bounds
            || canonical_du_delta.reconstruct()? != delta_bytes
        {
            return Err(anyhow!(
                "canonical P1 DU delta differs structurally from exact input delta"
            )
            .into());
        }

        let encode_started = Instant::now();
        let payloads = vec![
            (TRANSCRIPT_DELTA, delta_bytes.to_vec()),
            (DU_DICT_DELTA, encode_dictionary_delta(layer)?),
            (DU_STREAM_DELTA, encode_stream(&canonical_du_delta)?),
            (
                NATIVE_ID_DELTA,
                encode_native_id_delta(kept, &updated.direct_id.native_ids[kept..])?,
            ),
            (
                NATIVE_SPAN_DELTA,
                encode_native_span_delta(
                    kept,
                    artifact.candidate.repair_start_byte,
                    &updated.direct_id.native_spans[kept..],
                )?,
            ),
        ];
        let mut offset = initial_file_len;
        let mut descriptors = Vec::with_capacity(payloads.len());
        for (kind, payload) in &payloads {
            descriptors.push(SectionDescriptor {
                kind: *kind,
                flags: 0,
                offset,
                len: u64_from_usize(payload.len(), "journal payload length")?,
                hash: hash_bytes(payload),
            });
            offset = offset
                .checked_add(u64_from_usize(payload.len(), "journal payload length")?)
                .context("journal payload offset overflow")?;
        }
        let transcript_hash = parse_hash_hex(&updated.transcript_sha256)?;
        let mut meta = JournalMeta {
            sequence: updated.append_sequence,
            parent: self.head,
            base_checkpoint_root: self.checkpoint.root,
            result_transcript_len: u64_from_usize(
                updated.transcript_len,
                "result transcript length",
            )?,
            result_transcript_hash: transcript_hash,
            next_canonical_du_id: updated.dictionary_layers.next_id,
            provenance: update_provenance,
            descriptors,
            record_root: [0; 32],
        };
        meta.record_root = record_root(&meta)?;
        let meta_bytes = encode_meta(&meta)?;
        let footer = Footer {
            meta_offset: offset,
            meta_len: u64_from_usize(meta_bytes.len(), "PJR0 META length")?,
            meta_hash: hash_bytes(&meta_bytes),
            record_root: meta.record_root,
        };
        let footer_bytes = encode_footer(&footer);
        let mut body = Vec::new();
        for (_, payload) in &payloads {
            body.extend_from_slice(payload);
        }
        body.extend_from_slice(&meta_bytes);
        let journal_encode_wall_ns = encode_started.elapsed().as_nanos();

        // Oracle IDs do not exist until the candidate, next state, record root,
        // META, and footer have all been constructed.
        let transcript = updated.direct_id.du_state.reconstruct()?;
        let append_before_oracle_wall_ns = append_total_started.elapsed().as_nanos();
        let oracle_started = Instant::now();
        let oracle_ids = oracle_full_tokenize(&transcript)?;
        let oracle_full_tokenize_wall_ns = oracle_started.elapsed().as_nanos();
        let append_after_oracle_started = Instant::now();
        let ids_exact = artifact.candidate.ids == oracle_ids;
        if !ids_exact {
            return Err(anyhow!(
                "incremental P1 candidate differs from full-tokenization oracle; journal unchanged"
            )
            .into());
        }

        let pre_write_len = file.metadata()?.len();
        if pre_write_len != initial_file_len {
            let (actual, _, _) = current_disk_head(&self.journal_path, self.checkpoint.root)?;
            return Err(P1AppendError::StaleHead {
                expected: expected_head,
                actual,
            });
        }
        let append_started = Instant::now();
        file.write_all(&body)?;
        let journal_append_wall_ns = append_started.elapsed().as_nanos();
        let footer_started = Instant::now();
        file.write_all(&footer_bytes)?;
        file.sync_all()?;
        let footer_commit_wall_ns = footer_started.elapsed().as_nanos();

        // Reload the newly committed tail, verify its payload hashes, and apply
        // it independently before installing the in-memory canonical state.
        let mut verify_file = File::open(&self.journal_path)?;
        let verify_len = verify_file.metadata()?.len();
        let mut verify_bytes_read = 0;
        let located = discover_before(&mut verify_file, verify_len, &mut verify_bytes_read)?
            .context("new PJR0 footer was not discoverable after commit")?;
        if located.meta.record_root != meta.record_root || located.footer_end != verify_len {
            return Err(anyhow!("new PJR0 committed tip does not match encoded record").into());
        }
        let replayed = apply_record(
            &prior_snapshot,
            &self.checkpoint.root,
            &self.head,
            &located,
            &mut verify_file,
            &mut verify_bytes_read,
        )?;
        if replayed != artifact.updated_snapshot {
            return Err(anyhow!(
                "reloaded PJR0 record does not reconstruct the exact candidate state"
            )
            .into());
        }

        self.snapshot = replayed;
        self.head = meta.record_root;
        self.committed_end = located.footer_end;
        let proteus_p1_append_total_wall_ns = append_total_excluding_oracle(
            append_before_oracle_wall_ns,
            append_after_oracle_started.elapsed().as_nanos(),
        );
        let timings = P1AppendTimings {
            checkpoint_open_wall_ns: self.open_timings.checkpoint_open_wall_ns,
            journal_tail_discovery_wall_ns: self
                .open_timings
                .journal_tail_discovery_wall_ns
                .saturating_add(append_tail_ns),
            incremental_validation_wall_ns: artifact
                .timings
                .snapshot_validate_wall_ns
                .saturating_add(accepted_validation_wall_ns),
            delta_du_reconstruct_wall_ns: artifact.timings.delta_du_reconstruct_wall_ns,
            direct_id_candidate_prep_wall_ns: artifact.timings.direct_id_candidate_prep_wall_ns,
            native_span_delta_wall_ns: artifact.timings.native_span_update_wall_ns,
            dictionary_delta_wall_ns: artifact.timings.dictionary_layer_update_wall_ns,
            journal_encode_wall_ns,
            journal_append_wall_ns,
            footer_commit_wall_ns,
            proteus_p1_append_total_wall_ns,
            oracle_full_tokenize_wall_ns,
        };
        let bytes_appended = u64_from_usize(
            body.len()
                .checked_add(footer_bytes.len())
                .context("journal appended byte count overflow")?,
            "journal appended byte count",
        )?;
        let bytes_read = self
            .open_timings
            .bytes_read
            .checked_add(tail_bytes_read)
            .and_then(|value| value.checked_add(verify_bytes_read))
            .context("P1 receipt read byte count overflow")?;
        Ok(P1AppendReceipt {
            schema: "proteus-p1-append-receipt-v1",
            checkpoint_root: hash_hex(&self.checkpoint.root),
            previous_head: hash_hex(&expected_head),
            committed_head: hash_hex(&self.head),
            append_sequence: self.snapshot.append_sequence,
            ids_exact,
            candidate_constructed_before_oracle: true,
            oracle_used_for_candidate_construction: false,
            oracle_used_for_drop_selection: false,
            oracle_used_for_verification: true,
            transcript_len: self.snapshot.transcript_len,
            transcript_sha256: self.snapshot.transcript_sha256.clone(),
            dictionary_next_id: self.snapshot.dictionary_layers.next_id,
            new_dictionary_entries: artifact.new_dictionary_entries,
            bytes_read,
            bytes_appended,
            historical_canonical_payload_bytes_rewritten: 0,
            candidate: artifact.candidate,
            timings,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct P1Inspect {
    pub schema: &'static str,
    pub checkpoint_sequence: u64,
    pub checkpoint_root: String,
    pub committed_sequence: u64,
    pub committed_head: String,
    pub transcript_len: usize,
    pub transcript_sha256: String,
    pub dictionary_lineage: String,
    pub dictionary_next_id: u64,
    pub native_id_count: usize,
    pub native_span_count: usize,
    pub du_count: usize,
    pub committed_journal_bytes: u64,
}

pub fn inspect(checkpoint_path: &Path, journal_path: &Path) -> Result<P1Inspect> {
    let session = P1Session::open(checkpoint_path, journal_path)?;
    Ok(P1Inspect {
        schema: "proteus-p1-inspect-v1",
        checkpoint_sequence: session.checkpoint.sequence,
        checkpoint_root: hash_hex(&session.checkpoint.root),
        committed_sequence: session.snapshot.append_sequence,
        committed_head: hash_hex(&session.head),
        transcript_len: session.snapshot.transcript_len,
        transcript_sha256: session.snapshot.transcript_sha256.clone(),
        dictionary_lineage: session.snapshot.dictionary_lineage.clone(),
        dictionary_next_id: session.snapshot.dictionary_layers.next_id,
        native_id_count: session.snapshot.direct_id.native_ids.len(),
        native_span_count: session.snapshot.direct_id.native_spans.len(),
        du_count: session.snapshot.direct_id.du_state.ids.len(),
        committed_journal_bytes: session.committed_end,
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct P1AuditReport {
    pub schema: &'static str,
    pub valid: bool,
    pub checkpoint_root: String,
    pub committed_head: String,
    pub checkpoint_sequence: u64,
    pub committed_sequence: u64,
    pub audited_record_count: u64,
    pub transcript_len: usize,
    pub transcript_sha256: String,
    pub bytes_read: u64,
}

pub fn full_audit(checkpoint_path: &Path, journal_path: &Path) -> Result<P1AuditReport> {
    let checkpoint_bytes = fs::read(checkpoint_path)?;
    let checkpoint = decode_checkpoint(&checkpoint_bytes)?;
    let (records, discovery_bytes, _) = collect_lineage(journal_path)?;
    let mut snapshot = checkpoint.snapshot.clone();
    let mut head = checkpoint.root;
    let mut payload_bytes = 0;
    if !records.is_empty() {
        let mut file = File::open(journal_path)?;
        for record in &records {
            snapshot = apply_record(
                &snapshot,
                &checkpoint.root,
                &head,
                record,
                &mut file,
                &mut payload_bytes,
            )?;
            head = record.meta.record_root;
        }
    }
    let expected_count = snapshot
        .append_sequence
        .checked_sub(checkpoint.sequence)
        .context("audited sequence precedes checkpoint sequence")?;
    if expected_count != u64_from_usize(records.len(), "audited record count")? {
        bail!("journal record count and sequence range disagree");
    }
    Ok(P1AuditReport {
        schema: "proteus-p1-full-audit-v1",
        valid: true,
        checkpoint_root: hash_hex(&checkpoint.root),
        committed_head: hash_hex(&head),
        checkpoint_sequence: checkpoint.sequence,
        committed_sequence: snapshot.append_sequence,
        audited_record_count: expected_count,
        transcript_len: snapshot.transcript_len,
        transcript_sha256: snapshot.transcript_sha256,
        bytes_read: u64_from_usize(checkpoint_bytes.len(), "checkpoint audit bytes")?
            .checked_add(discovery_bytes)
            .and_then(|value| value.checked_add(payload_bytes))
            .context("audit byte count overflow")?,
    })
}

pub fn fold_checkpoint(
    checkpoint_path: &Path,
    journal_path: &Path,
    output_checkpoint: &Path,
) -> Result<P1Checkpoint> {
    let session = P1Session::open(checkpoint_path, journal_path)?;
    create_checkpoint(output_checkpoint, &session.snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "proteus-p1-{}-{}-{}",
                std::process::id(),
                serial,
                name
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn stream(ids: &[u64], parts: &[&[u8]]) -> LoadedStream {
        let mut end = 0;
        let mut bounds = vec![0];
        for part in parts {
            end += part.len();
            bounds.push(end);
        }
        LoadedStream {
            ids: ids.to_vec(),
            parts: parts.iter().map(|part| part.to_vec()).collect(),
            bounds,
        }
    }

    fn spans(bytes: &[u8]) -> Vec<TokenSpan> {
        bytes
            .iter()
            .enumerate()
            .map(|(index, byte)| TokenSpan {
                id: u64::from(*byte),
                byte_start: index,
                byte_end: index + 1,
                bytes_hex: bytes_to_hex(&[*byte]),
            })
            .collect()
    }

    fn snapshot() -> ProteusSnapshotV0 {
        ProteusSnapshotV0::initialize(
            b"ab",
            vec![u32::from(b'a'), u32::from(b'b')],
            spans(b"ab"),
            stream(&[41, 42], &[b"a", b"b"]),
            "lineage-test",
            ModelTokenizerIdentityV0 {
                model_id: "model".into(),
                model_provenance: "model-sha256".into(),
                tokenizer_id: "tokenizer".into(),
                tokenizer_provenance: "tokenizer-sha256".into(),
            },
            SnapshotProvenanceEventV0 {
                append_sequence: 0,
                unix_time_ns: 7,
                actor: "test".into(),
                source: "fixture".into(),
            },
        )
        .unwrap()
    }

    fn event(sequence: u64) -> SnapshotProvenanceEventV0 {
        SnapshotProvenanceEventV0 {
            append_sequence: sequence,
            unix_time_ns: u128::from(sequence) + 7,
            actor: "test".into(),
            source: "append".into(),
        }
    }

    fn repair(bytes: &[u8]) -> Result<Vec<NativeTokenPieceV0>> {
        Ok(bytes
            .iter()
            .map(|byte| NativeTokenPieceV0 {
                id: u32::from(*byte),
                bytes: vec![*byte],
            })
            .collect())
    }

    fn oracle(bytes: &[u8]) -> Result<Vec<u32>> {
        Ok(bytes.iter().map(|byte| u32::from(*byte)).collect())
    }

    fn setup(name: &str) -> (TestDir, PathBuf, PathBuf, P1Session) {
        let dir = TestDir::new(name);
        let checkpoint = dir.path("state.prt");
        let journal = dir.path("state.pjr");
        create_checkpoint(&checkpoint, &snapshot()).unwrap();
        let session = P1Session::open(&checkpoint, &journal).unwrap();
        (dir, checkpoint, journal, session)
    }

    fn append_byte(session: &mut P1Session, byte: u8) -> P1AppendReceipt {
        let sequence = session.snapshot.append_sequence + 1;
        let delta = [byte];
        session
            .append_verified(
                session.head(),
                &delta,
                &stream(&[999], &[&delta]),
                1,
                event(sequence),
                &mut repair,
                &mut oracle,
            )
            .unwrap()
    }

    #[test]
    fn prt0_deterministic_encode_decode() {
        let snapshot = snapshot();
        let (first, first_root) = encode_checkpoint(&snapshot).unwrap();
        let (second, second_root) = encode_checkpoint(&snapshot).unwrap();
        assert_eq!(first, second);
        assert_eq!(first_root, second_root);
        assert_eq!(decode_checkpoint(&first).unwrap().snapshot, snapshot);
    }

    #[test]
    fn prt0_root_is_stable_across_filename_and_location() {
        let one = TestDir::new("prt-location-one");
        let two = TestDir::new("prt-location-two");
        let left = create_checkpoint(&one.path("left.prt"), &snapshot()).unwrap();
        let right = create_checkpoint(&two.path("renamed.prt"), &snapshot()).unwrap();
        assert_eq!(left.root, right.root);
        assert_eq!(
            fs::read(one.path("left.prt")).unwrap(),
            fs::read(two.path("renamed.prt")).unwrap()
        );
    }

    #[test]
    fn corrupt_prt0_section_or_hash_is_rejected() {
        let (mut bytes, _) = encode_checkpoint(&snapshot()).unwrap();
        *bytes.last_mut().unwrap() ^= 0x80;
        assert!(decode_checkpoint(&bytes).is_err());
    }

    #[test]
    fn pjr0_first_and_later_parents_form_canonical_chain() {
        let (_dir, _checkpoint, journal, mut session) = setup("parents");
        let checkpoint_root = session.checkpoint_root();
        let first = append_byte(&mut session, b'c');
        assert_eq!(first.previous_head, hash_hex(&checkpoint_root));
        let first_root = session.head();
        let second = append_byte(&mut session, b'd');
        assert_eq!(second.previous_head, hash_hex(&first_root));
        assert_eq!(
            discover_journal_tail(&journal).unwrap().committed_sequence,
            Some(2)
        );
    }

    #[test]
    fn stale_expected_head_appends_zero_bytes() {
        let (_dir, _checkpoint, journal, mut session) = setup("stale");
        append_byte(&mut session, b'c');
        let before = fs::read(&journal).unwrap();
        let error = session
            .append_verified(
                session.checkpoint_root(),
                b"d",
                &stream(&[1], &[b"d"]),
                1,
                event(2),
                &mut repair,
                &mut oracle,
            )
            .unwrap_err();
        assert!(matches!(error, P1AppendError::StaleHead { .. }));
        assert_eq!(fs::read(&journal).unwrap(), before);
    }

    #[test]
    fn append_preserves_historical_prefix_byte_for_byte() {
        let (_dir, _checkpoint, journal, mut session) = setup("prefix");
        append_byte(&mut session, b'c');
        let prefix = fs::read(&journal).unwrap();
        let receipt = append_byte(&mut session, b'd');
        let after = fs::read(&journal).unwrap();
        assert_eq!(&after[..prefix.len()], prefix);
        assert_eq!(receipt.historical_canonical_payload_bytes_rewritten, 0);
    }

    #[test]
    fn exact_eof_tail_and_payload_magic_recovery() {
        let (_dir, _checkpoint, journal, mut session) = setup("tail-magic");
        let delta = b"PJR0";
        session
            .append_verified(
                session.head(),
                delta,
                &stream(&[5], &[delta]),
                1,
                event(1),
                &mut repair,
                &mut oracle,
            )
            .unwrap();
        let exact = discover_journal_tail(&journal).unwrap();
        assert_eq!(exact.trailing_uncommitted_bytes, 0);
        let committed = exact.committed_end;
        let mut file = OpenOptions::new().append(true).open(&journal).unwrap();
        file.write_all(b"noise-PJR0-not-a-footer").unwrap();
        drop(file);
        let recovered = discover_journal_tail(&journal).unwrap();
        assert_eq!(recovered.committed_end, committed);
        assert!(recovered.trailing_uncommitted_bytes > 0);
    }

    #[test]
    fn truncation_during_payload_meta_and_footer_recovers_previous_tip() {
        let (_dir, _checkpoint, journal, mut session) = setup("truncations");
        append_byte(&mut session, b'c');
        let prior = fs::read(&journal).unwrap();
        let prior_len = prior.len();
        append_byte(&mut session, b'd');
        let complete = fs::read(&journal).unwrap();
        let mut file = File::open(&journal).unwrap();
        let mut read = 0;
        let latest = discover_before(&mut file, complete.len() as u64, &mut read)
            .unwrap()
            .unwrap();
        let latest_meta_start =
            latest.footer_offset as usize - encode_meta(&latest.meta).unwrap().len();
        let cuts = [
            prior_len + 1,
            latest_meta_start + 1,
            latest.footer_offset as usize + PJR_FOOTER_LEN / 2,
        ];
        for cut in cuts {
            fs::write(&journal, &complete[..cut]).unwrap();
            let tail = discover_journal_tail(&journal).unwrap();
            assert_eq!(tail.committed_sequence, Some(1));
            assert_eq!(tail.committed_end as usize, prior_len);
        }
    }

    #[test]
    fn latest_earlier_complete_footer_is_selected() {
        let (_dir, _checkpoint, journal, mut session) = setup("earlier-footer");
        append_byte(&mut session, b'c');
        append_byte(&mut session, b'd');
        let second_len = fs::metadata(&journal).unwrap().len();
        append_byte(&mut session, b'e');
        let all = fs::read(&journal).unwrap();
        fs::write(&journal, &all[..all.len() - 17]).unwrap();
        let tail = discover_journal_tail(&journal).unwrap();
        assert_eq!(tail.committed_sequence, Some(2));
        assert_eq!(tail.committed_end, second_len);
    }

    #[test]
    fn malformed_meta_offset_length_and_corrupt_footer_are_rejected() {
        let (_dir, checkpoint, journal, mut session) = setup("bad-footer");
        append_byte(&mut session, b'c');
        let original = fs::read(&journal).unwrap();
        let footer_at = original.len() - PJR_FOOTER_LEN;

        for field_offset in [16, 24] {
            let mut bad_bounds = original.clone();
            bad_bounds[footer_at + field_offset..footer_at + field_offset + 8]
                .copy_from_slice(&u64::MAX.to_le_bytes());
            let checksum = hash_domain(
                b"PROTEUS:PJR0:FOOTER:V1\0",
                &[&bad_bounds[footer_at..footer_at + PJR_FOOTER_LEN - 32]],
            );
            bad_bounds[footer_at + PJR_FOOTER_LEN - 32..].copy_from_slice(&checksum);
            fs::write(&journal, bad_bounds).unwrap();
            assert_eq!(
                P1Session::open(&checkpoint, &journal)
                    .unwrap()
                    .snapshot()
                    .append_sequence,
                0
            );
        }

        let mut corrupt = original;
        *corrupt.last_mut().unwrap() ^= 1;
        fs::write(&journal, corrupt).unwrap();
        assert_eq!(
            P1Session::open(&checkpoint, &journal)
                .unwrap()
                .snapshot()
                .append_sequence,
            0
        );
    }

    #[test]
    fn malformed_section_overlap_is_rejected() {
        let (_dir, checkpoint, journal, mut session) = setup("overlap");
        append_byte(&mut session, b'c');
        let mut bytes = fs::read(&journal).unwrap();
        let mut file = File::open(&journal).unwrap();
        let mut read = 0;
        let located = discover_before(&mut file, bytes.len() as u64, &mut read)
            .unwrap()
            .unwrap();
        let mut meta = located.meta.clone();
        meta.descriptors[1].offset = meta.descriptors[0].offset;
        meta.record_root = record_root(&meta).unwrap();
        let meta_bytes = encode_meta(&meta).unwrap();
        let meta_start = located.footer_offset as usize - meta_bytes.len();
        bytes[meta_start..located.footer_offset as usize].copy_from_slice(&meta_bytes);
        let footer = encode_footer(&Footer {
            meta_offset: meta_start as u64,
            meta_len: meta_bytes.len() as u64,
            meta_hash: hash_bytes(&meta_bytes),
            record_root: meta.record_root,
        });
        bytes[located.footer_offset as usize..].copy_from_slice(&footer);
        fs::write(&journal, bytes).unwrap();
        assert_eq!(
            P1Session::open(&checkpoint, &journal)
                .unwrap()
                .snapshot()
                .append_sequence,
            0
        );
    }

    #[test]
    fn corrupt_section_hash_is_rejected() {
        let (_dir, checkpoint, journal, mut session) = setup("section-hash");
        append_byte(&mut session, b'c');
        let mut bytes = fs::read(&journal).unwrap();
        bytes[0] ^= 1;
        fs::write(&journal, bytes).unwrap();
        assert!(P1Session::open(&checkpoint, &journal).is_err());
    }

    #[test]
    fn journal_reconstructs_exact_snapshot_and_preserves_du_ids() {
        let (_dir, checkpoint, journal, mut session) = setup("reconstruct");
        append_byte(&mut session, b'c');
        append_byte(&mut session, b'd');
        let expected = session.snapshot.clone();
        let reopened = P1Session::open(&checkpoint, &journal).unwrap();
        assert_eq!(reopened.snapshot, expected);
        assert_eq!(reopened.snapshot.direct_id.du_state.ids, [41, 42, 43, 44]);
        assert_eq!(reopened.snapshot.dictionary_layers.next_id, 45);
    }

    #[test]
    fn duplicate_lexeme_reuses_id_and_new_ids_are_first_seen_max_plus_one() {
        let (_dir, _checkpoint, _journal, mut session) = setup("dict-order");
        let delta = b"aca";
        session
            .append_verified(
                session.head(),
                delta,
                &stream(&[9, 10, 9], &[b"a", b"c", b"a"]),
                1,
                event(1),
                &mut repair,
                &mut oracle,
            )
            .unwrap();
        assert_eq!(
            session.snapshot.direct_id.du_state.ids,
            [41, 42, 41, 43, 41]
        );
        assert_eq!(
            session.snapshot.dictionary_layers.deltas[0].entries[0].id,
            43
        );
    }

    #[test]
    fn warm_session_loads_before_loop_and_never_mutates_journal() {
        let (dir, checkpoint, journal, mut durable) = setup("warm-detached");
        append_byte(&mut durable, b'c');
        let accepted_head = durable.head();
        let journal_before = fs::read(&journal).unwrap();
        let mut warm = P1WarmSession::from_loaded(durable, accepted_head).unwrap();

        // Removing the original path names after construction makes any
        // checkpoint/journal reopen in the warm append deterministically fail.
        let moved_checkpoint = dir.path("loaded-before-loop.prt");
        let moved_journal = dir.path("loaded-before-loop.pjr");
        fs::rename(&checkpoint, &moved_checkpoint).unwrap();
        fs::rename(&journal, &moved_journal).unwrap();

        warm.append_verified(
            1,
            b"d",
            &stream(&[999], &[b"d"]),
            1,
            event(2),
            &mut repair,
            &mut oracle,
        )
        .unwrap();
        assert_eq!(
            warm.snapshot.direct_id.du_state.reconstruct().unwrap(),
            b"abcd"
        );
        assert_eq!(fs::read(moved_journal).unwrap(), journal_before);
        assert!(!checkpoint.exists());
        assert!(!journal.exists());
    }

    #[test]
    fn warm_appends_evolve_exact_state_without_renumbering_or_full_snapshot_clone() {
        let (_dir, _checkpoint, _journal, loaded) = setup("warm-sequential");
        let accepted_head = loaded.head();
        let mut warm = P1WarmSession::from_loaded(loaded, accepted_head).unwrap();
        let historical_part = warm.snapshot.direct_id.du_state.parts[0].as_ptr();
        let historical_lexeme = warm.snapshot.dictionary_layers.base.entries[0]
            .lexeme_bytes
            .as_ptr();
        let creation_actor = warm.snapshot.creation_provenance.actor.as_ptr();
        let oracle_calls = Cell::new(0usize);
        let mut exact_oracle = |bytes: &[u8]| {
            oracle_calls.set(oracle_calls.get() + 1);
            oracle(bytes)
        };
        let delta_du = stream(&[90, 91, 90], &[b"a", b"c", b"a"]);

        let first = warm
            .append_verified(
                1,
                b"aca",
                &delta_du,
                1,
                event(1),
                &mut repair,
                &mut exact_oracle,
            )
            .unwrap();
        let first_head = warm.head();
        let second = warm
            .append_verified(
                2,
                b"aca",
                &delta_du,
                1,
                event(2),
                &mut repair,
                &mut exact_oracle,
            )
            .unwrap();

        assert_eq!(oracle_calls.get(), 2);
        assert_eq!(first.append_sequence, 1);
        assert_eq!(second.append_sequence, 2);
        assert_eq!(first.new_dictionary_entries, 1);
        assert_eq!(second.new_dictionary_entries, 0);
        assert_eq!(
            warm.snapshot.direct_id.du_state.reconstruct().unwrap(),
            b"abacaaca"
        );
        assert_eq!(
            warm.snapshot.direct_id.du_state.ids,
            [41, 42, 41, 43, 41, 41, 43, 41]
        );
        assert_eq!(warm.snapshot.dictionary_layers.next_id, 44);
        assert_eq!(warm.snapshot.dictionary_layers.deltas[0].entries[0].id, 43);
        assert!(warm.snapshot.dictionary_layers.deltas[1].entries.is_empty());
        assert_ne!(warm.head(), accepted_head);
        assert_ne!(warm.head(), first_head);

        // Historical owned allocations remain the same allocations. The warm
        // path extends/truncates individual state vectors; it never clones and
        // replaces the complete historical Proteus snapshot.
        assert_eq!(
            warm.snapshot.direct_id.du_state.parts[0].as_ptr(),
            historical_part
        );
        assert_eq!(
            warm.snapshot.dictionary_layers.base.entries[0]
                .lexeme_bytes
                .as_ptr(),
            historical_lexeme
        );
        assert_eq!(
            warm.snapshot.creation_provenance.actor.as_ptr(),
            creation_actor
        );
    }

    #[test]
    fn warm_candidate_and_total_are_complete_before_oracle() {
        let (_dir, _checkpoint, _journal, loaded) = setup("warm-oracle-order");
        let accepted_head = loaded.head();
        let mut warm = P1WarmSession::from_loaded(loaded, accepted_head).unwrap();
        let order = Cell::new(0usize);
        let mut ordered_repair = |bytes: &[u8]| {
            assert_eq!(order.get(), 0);
            repair(bytes)
        };
        let pending = warm
            .advance_in_memory(
                1,
                b"c",
                &stream(&[9], &[b"c"]),
                1,
                event(1),
                &mut ordered_repair,
            )
            .unwrap();
        let frozen_total = pending.sample.warm_compute_total_wall_ns;
        let constructed_head = warm.head();
        assert_ne!(constructed_head, accepted_head);
        assert_eq!(pending.sample.oracle_full_tokenize_wall_ns, 0);
        order.set(1);
        let mut ordered_oracle = |bytes: &[u8]| {
            assert_eq!(order.get(), 1);
            order.set(2);
            oracle(bytes)
        };
        let sample = warm.verify_pending(pending, &mut ordered_oracle).unwrap();
        assert_eq!(order.get(), 2);
        assert_eq!(sample.warm_compute_total_wall_ns, frozen_total);
        assert_eq!(warm.head(), constructed_head);
    }

    #[test]
    fn fold_reconstructs_identical_state_without_renumbering() {
        let (dir, checkpoint, journal, mut session) = setup("fold");
        append_byte(&mut session, b'c');
        append_byte(&mut session, b'd');
        let expected = session.snapshot.clone();
        let folded_path = dir.path("folded.prt");
        let folded = fold_checkpoint(&checkpoint, &journal, &folded_path).unwrap();
        assert_eq!(folded.snapshot, expected);
        assert_eq!(folded.snapshot.direct_id.du_state.ids, [41, 42, 43, 44]);
        let new_journal = dir.path("folded.pjr");
        let reopened = P1Session::open(&folded_path, &new_journal).unwrap();
        assert_eq!(reopened.head(), folded.root);
    }

    #[test]
    fn logical_hashes_ignore_physical_offsets() {
        let descriptors = vec![
            SectionDescriptor {
                kind: CK_TRANSCRIPT,
                flags: 0,
                offset: 100,
                len: 7,
                hash: hash_bytes(b"content"),
            },
            SectionDescriptor {
                kind: CK_NATIVE_IDS,
                flags: 0,
                offset: 107,
                len: 4,
                hash: hash_bytes(b"ids!"),
            },
        ];
        let mut moved = descriptors.clone();
        moved[0].offset = 999_999;
        moved[1].offset = 42;
        moved.reverse();
        assert_eq!(
            checkpoint_root(3, &descriptors).unwrap(),
            checkpoint_root(3, &moved).unwrap()
        );
        let base = JournalMeta {
            sequence: 4,
            parent: [1; 32],
            base_checkpoint_root: [2; 32],
            result_transcript_len: 7,
            result_transcript_hash: [3; 32],
            next_canonical_du_id: 9,
            provenance: event(4),
            descriptors,
            record_root: [0; 32],
        };
        let mut relocated = base.clone();
        relocated.descriptors[0].offset = 42;
        relocated.descriptors[1].offset = 900;
        relocated.descriptors.reverse();
        assert_eq!(
            record_root(&base).unwrap(),
            record_root(&relocated).unwrap()
        );
    }

    #[test]
    fn candidate_is_constructed_before_oracle_and_total_excludes_oracle() {
        let (_dir, _checkpoint, _journal, mut session) = setup("oracle-order");
        let candidate_seen = Cell::new(false);
        let mut ordered_repair = |bytes: &[u8]| {
            candidate_seen.set(true);
            repair(bytes)
        };
        let mut ordered_oracle = |bytes: &[u8]| {
            assert!(candidate_seen.get());
            oracle(bytes)
        };
        let receipt = session
            .append_verified(
                session.head(),
                b"c",
                &stream(&[9], &[b"c"]),
                1,
                event(1),
                &mut ordered_repair,
                &mut ordered_oracle,
            )
            .unwrap();
        assert!(receipt.candidate_constructed_before_oracle);
        assert_eq!(append_total_excluding_oracle(11, 13), 24);
        assert_ne!(append_total_excluding_oracle(11, 13), 11 + 997 + 13);
        let measured_stages = receipt
            .timings
            .incremental_validation_wall_ns
            .saturating_add(receipt.timings.delta_du_reconstruct_wall_ns)
            .saturating_add(receipt.timings.direct_id_candidate_prep_wall_ns)
            .saturating_add(receipt.timings.native_span_delta_wall_ns)
            .saturating_add(receipt.timings.dictionary_delta_wall_ns)
            .saturating_add(receipt.timings.journal_encode_wall_ns)
            .saturating_add(receipt.timings.journal_append_wall_ns)
            .saturating_add(receipt.timings.footer_commit_wall_ns);
        assert!(receipt.timings.proteus_p1_append_total_wall_ns >= measured_stages);
    }

    #[test]
    fn full_audit_catches_historical_corruption() {
        let (_dir, checkpoint, journal, mut session) = setup("audit-corruption");
        append_byte(&mut session, b'c');
        let first_end = session.committed_end() as usize;
        append_byte(&mut session, b'd');
        assert!(full_audit(&checkpoint, &journal).unwrap().valid);
        let mut bytes = fs::read(&journal).unwrap();
        assert!(first_end < bytes.len());
        bytes[0] ^= 1;
        fs::write(&journal, bytes).unwrap();
        assert!(full_audit(&checkpoint, &journal).is_err());
    }
}
