//! Proteus P0 snapshot/delta orchestration over sealed Nereids Direct-ID V2.
//!
//! This crate operates on exact, already-rendered conversation bytes. It does
//! not render chat templates and it does not make bounded model context
//! infinite. Candidate construction and the full-tokenization oracle are
//! deliberately separate APIs.

use anyhow::{bail, Context, Result};
use nereids::direct_id::{construct_du_aware_repair_candidate, DirectIdSnapshotV2};
use nereids::{sha256_hex, LoadedStream, TokenSpan};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const SNAPSHOT_SCHEMA: &str = "proteus-snapshot-v0";
pub const SNAPSHOT_VERSION: u32 = 0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelTokenizerIdentityV0 {
    pub model_id: String,
    pub model_provenance: String,
    pub tokenizer_id: String,
    pub tokenizer_provenance: String,
}

impl ModelTokenizerIdentityV0 {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("model_id", &self.model_id),
            ("model_provenance", &self.model_provenance),
            ("tokenizer_id", &self.tokenizer_id),
            ("tokenizer_provenance", &self.tokenizer_provenance),
        ] {
            if value.trim().is_empty() {
                bail!("model/tokenizer identity field {name} must not be empty");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotProvenanceEventV0 {
    pub append_sequence: u64,
    pub unix_time_ns: u128,
    pub actor: String,
    pub source: String,
}

impl SnapshotProvenanceEventV0 {
    pub fn now(append_sequence: u64, actor: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            append_sequence,
            unix_time_ns: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
            actor: actor.into(),
            source: source.into(),
        }
    }

    fn validate_for_sequence(&self, sequence: u64) -> Result<()> {
        if self.append_sequence != sequence {
            bail!(
                "provenance event sequence {} does not match expected sequence {sequence}",
                self.append_sequence
            );
        }
        if self.actor.trim().is_empty() || self.source.trim().is_empty() {
            bail!("snapshot provenance actor and source must not be empty");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DictionaryEntryV0 {
    pub id: u64,
    /// Exact DU lexeme bytes. No normalization is applied.
    pub lexeme_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DictionaryBaseLayerV0 {
    pub entries: Vec<DictionaryEntryV0>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DictionaryDeltaLayerV0 {
    pub append_sequence: u64,
    pub entries: Vec<DictionaryEntryV0>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuDictionaryLayersV0 {
    /// Stable identity shared by every snapshot in this dictionary lineage.
    pub lineage: String,
    pub next_id: u64,
    pub base: DictionaryBaseLayerV0,
    pub deltas: Vec<DictionaryDeltaLayerV0>,
}

impl DuDictionaryLayersV0 {
    /// Capture the exact ID/lexeme associations present in the initial DU
    /// stream as the immutable base layer for this dictionary lineage.
    pub fn from_base_stream(lineage: impl Into<String>, stream: &LoadedStream) -> Result<Self> {
        stream.validate_structure()?;
        let lineage = lineage.into();
        if lineage.trim().is_empty() {
            bail!("dictionary lineage must not be empty");
        }
        let mut by_id = BTreeMap::<u64, Vec<u8>>::new();
        let mut by_bytes = HashMap::<Vec<u8>, u64>::new();
        validate_stream_associations(stream)?;

        for (&id, bytes) in stream.ids.iter().zip(&stream.parts) {
            if bytes.is_empty() {
                bail!("DU dictionary lexemes must not be empty");
            }
            if let Some(existing) = by_id.get(&id) {
                if existing != bytes {
                    bail!("base DU ID {id} is associated with more than one lexeme");
                }
            } else {
                by_id.insert(id, bytes.clone());
            }
            if let Some(existing_id) = by_bytes.get(bytes) {
                if *existing_id != id {
                    bail!("one base DU lexeme is associated with IDs {existing_id} and {id}");
                }
            } else {
                by_bytes.insert(bytes.clone(), id);
            }
        }

        let next_id = match by_id.keys().next_back().copied() {
            Some(id) => id
                .checked_add(1)
                .context("base DU dictionary ID overflow")?,
            None => 0,
        };
        let dictionary = Self {
            lineage,
            next_id,
            base: DictionaryBaseLayerV0 {
                entries: by_id
                    .into_iter()
                    .map(|(id, lexeme_bytes)| DictionaryEntryV0 { id, lexeme_bytes })
                    .collect(),
            },
            deltas: Vec::new(),
        };
        dictionary.validate()?;
        Ok(dictionary)
    }

    pub fn validate(&self) -> Result<()> {
        if self.lineage.trim().is_empty() {
            bail!("dictionary lineage must not be empty");
        }
        let mut ids = HashSet::new();
        let mut lexemes = HashSet::new();
        let mut largest_id = None::<u64>;

        let mut previous_base_id = None;
        for entry in &self.base.entries {
            if entry.lexeme_bytes.is_empty() {
                bail!("base dictionary contains an empty lexeme");
            }
            if previous_base_id.is_some_and(|previous| entry.id <= previous) {
                bail!("base dictionary entries must be strictly ordered by ID");
            }
            previous_base_id = Some(entry.id);
            insert_unique_entry(entry, &mut ids, &mut lexemes)?;
            largest_id = Some(largest_id.map_or(entry.id, |largest| largest.max(entry.id)));
        }

        let mut expected_next = largest_id
            .map(|id| id.checked_add(1).context("dictionary ID overflow"))
            .transpose()?
            .unwrap_or(0);
        for (index, layer) in self.deltas.iter().enumerate() {
            let expected_sequence = u64::try_from(index)
                .context("too many dictionary delta layers")?
                .checked_add(1)
                .context("dictionary append sequence overflow")?;
            if layer.append_sequence != expected_sequence {
                bail!(
                    "dictionary delta layer sequence {} (expected {expected_sequence})",
                    layer.append_sequence
                );
            }
            for entry in &layer.entries {
                if entry.id != expected_next {
                    bail!(
                        "dictionary delta ID {} (expected {expected_next})",
                        entry.id
                    );
                }
                if entry.lexeme_bytes.is_empty() {
                    bail!("dictionary delta contains an empty lexeme");
                }
                insert_unique_entry(entry, &mut ids, &mut lexemes)?;
                expected_next = expected_next
                    .checked_add(1)
                    .context("dictionary delta ID overflow")?;
            }
        }
        if self.next_id != expected_next {
            bail!(
                "dictionary next_id {} does not match validated next ID {expected_next}",
                self.next_id
            );
        }
        Ok(())
    }

    pub fn lookup_exact(&self, lexeme_bytes: &[u8]) -> Option<u64> {
        self.base
            .entries
            .iter()
            .chain(self.deltas.iter().flat_map(|layer| &layer.entries))
            .find(|entry| entry.lexeme_bytes == lexeme_bytes)
            .map(|entry| entry.id)
    }

    /// Add one deterministic layer and return the delta with canonical IDs.
    /// New IDs follow exact first-seen byte order in `delta`.
    pub fn append_delta(
        &mut self,
        append_sequence: u64,
        delta: &LoadedStream,
    ) -> Result<LoadedStream> {
        self.validate()?;
        delta.validate_structure()?;
        validate_stream_associations(delta)?;
        let expected_sequence = u64::try_from(self.deltas.len())
            .context("too many dictionary delta layers")?
            .checked_add(1)
            .context("dictionary append sequence overflow")?;
        if append_sequence != expected_sequence {
            bail!("dictionary append sequence {append_sequence} (expected {expected_sequence})");
        }

        let mut known = HashMap::<Vec<u8>, u64>::new();
        for entry in self
            .base
            .entries
            .iter()
            .chain(self.deltas.iter().flat_map(|layer| &layer.entries))
        {
            known.insert(entry.lexeme_bytes.clone(), entry.id);
        }

        let mut canonical_ids = Vec::with_capacity(delta.ids.len());
        let mut appended = Vec::new();
        let prior_next_id = self.next_id;
        let mut next_id = prior_next_id;
        // Assignment order is the stored delta order. Sorting is permitted
        // only when building separate derived lookup metadata.
        for bytes in &delta.parts {
            if bytes.is_empty() {
                bail!("DU dictionary lexemes must not be empty");
            }
            let id = if let Some(id) = known.get(bytes).copied() {
                id
            } else {
                let id = next_id;
                next_id = next_id
                    .checked_add(1)
                    .context("dictionary delta ID overflow")?;
                known.insert(bytes.clone(), id);
                appended.push(DictionaryEntryV0 {
                    id,
                    lexeme_bytes: bytes.clone(),
                });
                id
            };
            canonical_ids.push(id);
        }
        self.next_id = next_id;
        self.deltas.push(DictionaryDeltaLayerV0 {
            append_sequence,
            entries: appended,
        });
        if let Err(error) = self.validate() {
            self.deltas.pop();
            self.next_id = prior_next_id;
            return Err(error);
        }

        Ok(LoadedStream {
            ids: canonical_ids,
            parts: delta.parts.clone(),
            bounds: delta.bounds.clone(),
        })
    }

    /// Build lexicographically sorted lookup metadata without changing the
    /// canonical ID associated with any lexeme or mutating persisted layers.
    pub fn derived_sorted_lookup(&self) -> Result<Vec<DictionaryEntryV0>> {
        self.validate()?;
        let mut entries = self.all_entries().into_iter().cloned().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.lexeme_bytes.cmp(&right.lexeme_bytes));
        Ok(entries)
    }

    fn all_entries(&self) -> Vec<&DictionaryEntryV0> {
        self.base
            .entries
            .iter()
            .chain(self.deltas.iter().flat_map(|layer| &layer.entries))
            .collect()
    }

    fn validate_stream(&self, stream: &LoadedStream) -> Result<()> {
        stream.validate_structure()?;
        let by_lexeme: HashMap<&[u8], u64> = self
            .base
            .entries
            .iter()
            .chain(self.deltas.iter().flat_map(|layer| &layer.entries))
            .map(|entry| (entry.lexeme_bytes.as_slice(), entry.id))
            .collect::<HashMap<_, _>>();
        for (&id, bytes) in stream.ids.iter().zip(&stream.parts) {
            let Some(expected) = by_lexeme.get(bytes.as_slice()).copied() else {
                bail!(
                    "DU stream lexeme is absent from dictionary lineage {}",
                    self.lineage
                );
            };
            if id != expected {
                bail!(
                    "DU stream ID {id} does not match canonical ID {expected} for exact lexeme bytes"
                );
            }
        }
        Ok(())
    }
}

fn validate_stream_associations(stream: &LoadedStream) -> Result<()> {
    let mut by_id = HashMap::<u64, &[u8]>::new();
    for (&id, bytes) in stream.ids.iter().zip(&stream.parts) {
        if let Some(existing) = by_id.insert(id, bytes) {
            if existing != bytes.as_slice() {
                bail!("DU input ID {id} is associated with more than one exact lexeme");
            }
        }
    }
    Ok(())
}

fn insert_unique_entry(
    entry: &DictionaryEntryV0,
    ids: &mut HashSet<u64>,
    lexemes: &mut HashSet<Vec<u8>>,
) -> Result<()> {
    if !ids.insert(entry.id) {
        bail!("dictionary contains duplicate ID {}", entry.id);
    }
    if !lexemes.insert(entry.lexeme_bytes.clone()) {
        bail!("dictionary contains a duplicate exact lexeme");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProteusSnapshotV0 {
    pub schema: String,
    pub version: u32,
    pub append_sequence: u64,
    pub dictionary_lineage: String,
    pub model_tokenizer: ModelTokenizerIdentityV0,
    pub transcript_len: usize,
    pub transcript_sha256: String,
    /// Nereids remains the owner of native-ID/span and DU reconstruction invariants.
    pub direct_id: DirectIdSnapshotV2,
    pub dictionary_layers: DuDictionaryLayersV0,
    pub creation_provenance: SnapshotProvenanceEventV0,
    pub update_provenance: Vec<SnapshotProvenanceEventV0>,
}

impl ProteusSnapshotV0 {
    pub fn initialize(
        prefix: &[u8],
        native_ids: Vec<u32>,
        native_spans: Vec<TokenSpan>,
        prefix_du: LoadedStream,
        dictionary_lineage: impl Into<String>,
        model_tokenizer: ModelTokenizerIdentityV0,
        creation_provenance: SnapshotProvenanceEventV0,
    ) -> Result<Self> {
        if creation_provenance.append_sequence != 0 {
            bail!("snapshot creation provenance must use append sequence zero");
        }
        let dictionary_lineage = dictionary_lineage.into();
        let dictionary_layers =
            DuDictionaryLayersV0::from_base_stream(dictionary_lineage.clone(), &prefix_du)?;
        let direct_id = DirectIdSnapshotV2::new(prefix, native_ids, native_spans, prefix_du)
            .map_err(anyhow::Error::new)?;
        let snapshot = Self {
            schema: SNAPSHOT_SCHEMA.to_string(),
            version: SNAPSHOT_VERSION,
            append_sequence: 0,
            dictionary_lineage,
            model_tokenizer,
            transcript_len: prefix.len(),
            transcript_sha256: sha256_hex(prefix),
            direct_id,
            dictionary_layers,
            creation_provenance,
            update_provenance: Vec::new(),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != SNAPSHOT_SCHEMA || self.version != SNAPSHOT_VERSION {
            bail!(
                "unsupported Proteus snapshot schema/version {}/{}",
                self.schema,
                self.version
            );
        }
        self.model_tokenizer.validate()?;
        self.creation_provenance.validate_for_sequence(0)?;
        if self.update_provenance.len()
            != usize::try_from(self.append_sequence)
                .context("append sequence does not fit usize")?
        {
            bail!("snapshot update provenance count does not match append sequence");
        }
        for (index, event) in self.update_provenance.iter().enumerate() {
            event.validate_for_sequence(
                u64::try_from(index)
                    .context("too many provenance events")?
                    .checked_add(1)
                    .context("provenance sequence overflow")?,
            )?;
        }
        self.direct_id.validate().map_err(anyhow::Error::new)?;
        if self.transcript_len != self.direct_id.prefix_len {
            bail!("Proteus transcript length disagrees with Direct-ID V2");
        }
        if self.transcript_sha256 != self.direct_id.prefix_sha256 {
            bail!("Proteus transcript hash disagrees with Direct-ID V2");
        }
        self.dictionary_layers.validate()?;
        if self.dictionary_lineage != self.dictionary_layers.lineage {
            bail!("snapshot dictionary lineage disagrees with dictionary layers");
        }
        if self.dictionary_layers.deltas.len()
            != usize::try_from(self.append_sequence)
                .context("append sequence does not fit usize")?
        {
            bail!("dictionary delta layer count does not match append sequence");
        }
        self.dictionary_layers
            .validate_stream(&self.direct_id.du_state)?;
        Ok(())
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn from_json_slice(bytes: &[u8]) -> Result<Self> {
        let snapshot: Self =
            serde_json::from_slice(bytes).context("deserialize Proteus snapshot")?;
        snapshot.validate().context("validate Proteus snapshot")?;
        Ok(snapshot)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTokenPieceV0 {
    pub id: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProteusResumeTimingsV0 {
    pub snapshot_deserialize_wall_ns: u128,
    pub snapshot_validate_wall_ns: u128,
    pub delta_du_reconstruct_wall_ns: u128,
    pub direct_id_candidate_prep_wall_ns: u128,
    pub native_span_update_wall_ns: u128,
    pub dictionary_layer_update_wall_ns: u128,
    pub snapshot_serialize_wall_ns: u128,
    pub proteus_resume_total_wall_ns: u128,
    pub oracle_full_tokenize_wall_ns: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncrementalCandidateV0 {
    pub ids: Vec<u32>,
    pub native_spans: Vec<TokenSpan>,
    pub fixed_repair_depth: usize,
    pub kept_snapshot_token_count: usize,
    pub repair_start_byte: usize,
    pub repair_window_bytes: usize,
    pub repair_token_count: usize,
    pub constructed_without_oracle_ids: bool,
}

#[derive(Debug, Clone)]
pub struct ResumeArtifactV0 {
    pub previous_append_sequence: u64,
    pub updated_snapshot: ProteusSnapshotV0,
    pub serialized_snapshot: Vec<u8>,
    pub candidate: IncrementalCandidateV0,
    pub new_dictionary_entries: usize,
    pub timings: ProteusResumeTimingsV0,
}

#[derive(Debug, Clone)]
pub struct VerifiedResumeV0 {
    pub artifact: ResumeArtifactV0,
    pub oracle_ids: Vec<u32>,
    pub ids_exact: bool,
}

pub fn resume_snapshot_json<TokenizeRepair>(
    snapshot_json: &[u8],
    delta_bytes: &[u8],
    delta_du: &LoadedStream,
    fixed_repair_depth: usize,
    update_provenance: SnapshotProvenanceEventV0,
    tokenize_repair_window: &mut TokenizeRepair,
) -> Result<ResumeArtifactV0>
where
    TokenizeRepair: FnMut(&[u8]) -> Result<Vec<NativeTokenPieceV0>>,
{
    let total_started = Instant::now();
    let deserialize_started = Instant::now();
    let snapshot: ProteusSnapshotV0 =
        serde_json::from_slice(snapshot_json).context("deserialize Proteus snapshot")?;
    let mut timings = ProteusResumeTimingsV0 {
        snapshot_deserialize_wall_ns: deserialize_started.elapsed().as_nanos(),
        ..ProteusResumeTimingsV0::default()
    };
    resume_inner(
        snapshot,
        delta_bytes,
        delta_du,
        fixed_repair_depth,
        update_provenance,
        tokenize_repair_window,
        total_started,
        &mut timings,
    )
}

pub fn resume_snapshot_in_memory<TokenizeRepair>(
    snapshot: &ProteusSnapshotV0,
    delta_bytes: &[u8],
    delta_du: &LoadedStream,
    fixed_repair_depth: usize,
    update_provenance: SnapshotProvenanceEventV0,
    tokenize_repair_window: &mut TokenizeRepair,
) -> Result<ResumeArtifactV0>
where
    TokenizeRepair: FnMut(&[u8]) -> Result<Vec<NativeTokenPieceV0>>,
{
    let total_started = Instant::now();
    let mut timings = ProteusResumeTimingsV0::default();
    resume_inner(
        snapshot.clone(),
        delta_bytes,
        delta_du,
        fixed_repair_depth,
        update_provenance,
        tokenize_repair_window,
        total_started,
        &mut timings,
    )
}

#[allow(clippy::too_many_arguments)]
fn resume_inner<TokenizeRepair>(
    snapshot: ProteusSnapshotV0,
    delta_bytes: &[u8],
    delta_du: &LoadedStream,
    fixed_repair_depth: usize,
    update_provenance: SnapshotProvenanceEventV0,
    tokenize_repair_window: &mut TokenizeRepair,
    total_started: Instant,
    timings: &mut ProteusResumeTimingsV0,
) -> Result<ResumeArtifactV0>
where
    TokenizeRepair: FnMut(&[u8]) -> Result<Vec<NativeTokenPieceV0>>,
{
    let validate_started = Instant::now();
    snapshot.validate()?;
    timings.snapshot_validate_wall_ns = validate_started.elapsed().as_nanos();

    let next_sequence = snapshot
        .append_sequence
        .checked_add(1)
        .context("snapshot append sequence overflow")?;
    update_provenance.validate_for_sequence(next_sequence)?;

    let delta_started = Instant::now();
    let reconstructed_delta = delta_du
        .reconstruct()
        .context("reconstruct delta DU state")?;
    if reconstructed_delta != delta_bytes {
        bail!("delta DU state does not reconstruct the supplied delta bytes exactly");
    }
    timings.delta_du_reconstruct_wall_ns = delta_started.elapsed().as_nanos();

    // Nereids receives IDs only. Proteus retains the exact pieces returned by
    // the same repair callback so shifted native spans can be rebuilt without
    // detokenizing the old prefix.
    let mut repair_pieces = None::<Vec<NativeTokenPieceV0>>;
    let mut nereids_callback = |window: &[u8]| -> std::result::Result<Vec<u32>, String> {
        let pieces = tokenize_repair_window(window).map_err(|error| error.to_string())?;
        let ids = pieces.iter().map(|piece| piece.id).collect();
        repair_pieces = Some(pieces);
        Ok(ids)
    };
    let candidate_started = Instant::now();
    let (nereids_candidate, _) = construct_du_aware_repair_candidate(
        &snapshot.direct_id,
        delta_du,
        fixed_repair_depth,
        &mut nereids_callback,
    )
    .map_err(anyhow::Error::new)?;
    timings.direct_id_candidate_prep_wall_ns = candidate_started.elapsed().as_nanos();
    let repair_pieces =
        repair_pieces.context("native repair tokenizer returned no token pieces")?;

    let span_started = Instant::now();
    let mut repair_window = snapshot
        .direct_id
        .du_state
        .reconstruct_range(nereids_candidate.repair_start_byte, snapshot.transcript_len)?;
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
    if &nereids_candidate.ids[nereids_candidate.kept_snapshot_token_count..]
        != repair_ids.as_slice()
    {
        bail!("native repair token pieces disagree with the Direct-ID candidate suffix");
    }

    let mut native_spans =
        snapshot.direct_id.native_spans[..nereids_candidate.kept_snapshot_token_count].to_vec();
    let mut byte_cursor = nereids_candidate.repair_start_byte;
    for piece in &repair_pieces {
        if piece.bytes.is_empty() {
            bail!("native repair tokenizer returned an empty token piece");
        }
        let byte_end = byte_cursor
            .checked_add(piece.bytes.len())
            .context("native token span byte offset overflow")?;
        native_spans.push(TokenSpan {
            id: u64::from(piece.id),
            byte_start: byte_cursor,
            byte_end,
            bytes_hex: nereids::bytes_to_hex(&piece.bytes),
        });
        byte_cursor = byte_end;
    }
    let expected_len = snapshot
        .transcript_len
        .checked_add(delta_bytes.len())
        .context("transcript byte length overflow")?;
    if byte_cursor != expected_len {
        bail!("shifted native spans end at {byte_cursor}, expected {expected_len}");
    }
    timings.native_span_update_wall_ns = span_started.elapsed().as_nanos();

    let dictionary_started = Instant::now();
    let mut dictionary_layers = snapshot.dictionary_layers.clone();
    let canonical_delta = dictionary_layers.append_delta(next_sequence, delta_du)?;
    let combined_du = snapshot.direct_id.du_state.concatenate(&canonical_delta)?;
    let new_dictionary_entries = dictionary_layers
        .deltas
        .last()
        .context("dictionary append produced no delta layer")?
        .entries
        .len();
    timings.dictionary_layer_update_wall_ns = dictionary_started.elapsed().as_nanos();

    let transcript = combined_du.reconstruct()?;
    if transcript.len() != expected_len || !transcript.ends_with(delta_bytes) {
        bail!("combined DU state does not reconstruct the expected appended transcript");
    }
    let direct_id = DirectIdSnapshotV2::new(
        &transcript,
        nereids_candidate.ids.clone(),
        native_spans.clone(),
        combined_du,
    )
    .map_err(anyhow::Error::new)?;
    let mut update_provenance_log = snapshot.update_provenance.clone();
    update_provenance_log.push(update_provenance);
    let updated_snapshot = ProteusSnapshotV0 {
        schema: SNAPSHOT_SCHEMA.to_string(),
        version: SNAPSHOT_VERSION,
        append_sequence: next_sequence,
        dictionary_lineage: snapshot.dictionary_lineage.clone(),
        model_tokenizer: snapshot.model_tokenizer.clone(),
        transcript_len: transcript.len(),
        transcript_sha256: sha256_hex(&transcript),
        direct_id,
        dictionary_layers,
        creation_provenance: snapshot.creation_provenance.clone(),
        update_provenance: update_provenance_log,
    };
    updated_snapshot.validate()?;

    let candidate = IncrementalCandidateV0 {
        ids: nereids_candidate.ids,
        native_spans,
        fixed_repair_depth,
        kept_snapshot_token_count: nereids_candidate.kept_snapshot_token_count,
        repair_start_byte: nereids_candidate.repair_start_byte,
        repair_window_bytes: nereids_candidate.repair_window_bytes,
        repair_token_count: nereids_candidate.repair_token_count,
        constructed_without_oracle_ids: true,
    };

    let serialize_started = Instant::now();
    let serialized_snapshot = updated_snapshot.to_json_vec()?;
    timings.snapshot_serialize_wall_ns = serialize_started.elapsed().as_nanos();
    timings.proteus_resume_total_wall_ns = total_started.elapsed().as_nanos();

    Ok(ResumeArtifactV0 {
        previous_append_sequence: snapshot.append_sequence,
        updated_snapshot,
        serialized_snapshot,
        candidate,
        new_dictionary_entries,
        timings: timings.clone(),
    })
}

/// Consult the full-tokenization oracle only after a complete incremental
/// artifact exists. Oracle time is never added to `proteus_resume_total_wall_ns`.
pub fn verify_with_full_tokenize_oracle<Oracle>(
    mut artifact: ResumeArtifactV0,
    oracle_full_tokenize: &mut Oracle,
) -> Result<VerifiedResumeV0>
where
    Oracle: FnMut(&[u8]) -> Result<Vec<u32>>,
{
    let transcript = artifact.updated_snapshot.direct_id.du_state.reconstruct()?;
    let resume_total_before_oracle = artifact.timings.proteus_resume_total_wall_ns;
    let oracle_started = Instant::now();
    let oracle_ids = oracle_full_tokenize(&transcript)?;
    artifact.timings.oracle_full_tokenize_wall_ns = oracle_started.elapsed().as_nanos();
    if artifact.timings.proteus_resume_total_wall_ns != resume_total_before_oracle {
        bail!("internal timing error: oracle changed Proteus resume total");
    }
    let ids_exact = artifact.candidate.ids == oracle_ids;
    Ok(VerifiedResumeV0 {
        artifact,
        oracle_ids,
        ids_exact,
    })
}

pub fn load_snapshot(path: impl AsRef<Path>) -> Result<ProteusSnapshotV0> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| format!("read snapshot {}", path.display()))?;
    ProteusSnapshotV0::from_json_slice(&bytes)
        .with_context(|| format!("load snapshot {}", path.display()))
}

pub fn persist_snapshot(path: impl AsRef<Path>, snapshot: &ProteusSnapshotV0) -> Result<()> {
    let bytes = snapshot.to_json_vec()?;
    persist_bytes_atomically(path.as_ref(), &bytes)
}

pub fn persist_snapshot_bytes(path: impl AsRef<Path>, bytes: &[u8]) -> Result<()> {
    ProteusSnapshotV0::from_json_slice(bytes)
        .context("refuse to persist invalid Proteus snapshot bytes")?;
    persist_bytes_atomically(path.as_ref(), bytes)
}

fn persist_bytes_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)
            .with_context(|| format!("create snapshot directory {}", parent.display()))?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("snapshot path must have a UTF-8 file name")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary_name = format!(".{file_name}.tmp-{}-{nonce}", std::process::id());
    let temporary_path: PathBuf = path.with_file_name(temporary_name);
    let write_result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .with_context(|| format!("create temporary snapshot {}", temporary_path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write temporary snapshot {}", temporary_path.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temporary snapshot {}", temporary_path.display()))?;
        fs::rename(&temporary_path, path)
            .with_context(|| format!("install snapshot {}", path.display()))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    fn stream(ids: &[u64], parts: &[&[u8]]) -> LoadedStream {
        let mut bounds = vec![0];
        for part in parts {
            bounds.push(bounds.last().copied().unwrap() + part.len());
        }
        LoadedStream {
            ids: ids.to_vec(),
            parts: parts.iter().map(|part| part.to_vec()).collect(),
            bounds,
        }
    }

    fn identity() -> ModelTokenizerIdentityV0 {
        ModelTokenizerIdentityV0 {
            model_id: "test-model".into(),
            model_provenance: "sha256:test-weights".into(),
            tokenizer_id: "test-tokenizer".into(),
            tokenizer_provenance: "test:tokenizer-config".into(),
        }
    }

    fn provenance(sequence: u64) -> SnapshotProvenanceEventV0 {
        SnapshotProvenanceEventV0 {
            append_sequence: sequence,
            unix_time_ns: u128::from(sequence),
            actor: "test".into(),
            source: format!("test:event-{sequence}"),
        }
    }

    fn snapshot() -> ProteusSnapshotV0 {
        ProteusSnapshotV0::initialize(
            b"abcd",
            vec![10, 20],
            vec![
                TokenSpan {
                    id: 10,
                    byte_start: 0,
                    byte_end: 2,
                    bytes_hex: "6162".into(),
                },
                TokenSpan {
                    id: 20,
                    byte_start: 2,
                    byte_end: 4,
                    bytes_hex: "6364".into(),
                },
            ],
            stream(&[4, 8], &[b"abc", b"d"]),
            "test-lineage",
            identity(),
            provenance(0),
        )
        .unwrap()
    }

    fn append_bang(snapshot: &ProteusSnapshotV0) -> ResumeArtifactV0 {
        let delta = stream(&[90], &[b"!"]);
        let mut tokenize = |bytes: &[u8]| -> Result<Vec<NativeTokenPieceV0>> {
            assert_eq!(bytes, b"cd!");
            Ok(vec![
                NativeTokenPieceV0 {
                    id: 30,
                    bytes: b"c".to_vec(),
                },
                NativeTokenPieceV0 {
                    id: 31,
                    bytes: b"d!".to_vec(),
                },
            ])
        };
        resume_snapshot_in_memory(snapshot, b"!", &delta, 1, provenance(1), &mut tokenize).unwrap()
    }

    #[test]
    fn dictionary_append_is_deterministic_first_seen_order() {
        let mut dictionary =
            DuDictionaryLayersV0::from_base_stream("first-seen", &stream(&[0], &[b"base"]))
                .unwrap();
        let canonical = dictionary
            .append_delta(1, &stream(&[50, 51, 50, 52], &[b"z", b"a", b"z", b"m"]))
            .unwrap();
        assert_eq!(canonical.ids, [1, 2, 1, 3]);
        assert_eq!(
            dictionary.deltas[0]
                .entries
                .iter()
                .map(|entry| (entry.id, entry.lexeme_bytes.as_slice()))
                .collect::<Vec<_>>(),
            [
                (1, b"z".as_slice()),
                (2, b"a".as_slice()),
                (3, b"m".as_slice())
            ]
        );
    }

    #[test]
    fn dictionary_reuses_existing_lexemes() {
        let mut dictionary =
            DuDictionaryLayersV0::from_base_stream("existing", &stream(&[4], &[b"same"])).unwrap();
        let first = dictionary
            .append_delta(1, &stream(&[99, 100], &[b"same", b"new"]))
            .unwrap();
        assert_eq!(first.ids, [4, 5]);
        assert_eq!(dictionary.deltas[0].entries.len(), 1);

        let second = dictionary
            .append_delta(2, &stream(&[7, 8], &[b"new", b"same"]))
            .unwrap();
        assert_eq!(second.ids, [5, 4]);
        assert!(dictionary.deltas[1].entries.is_empty());
    }

    #[test]
    fn dictionary_reuses_duplicate_lexemes_within_one_delta() {
        let mut dictionary =
            DuDictionaryLayersV0::from_base_stream("duplicates", &stream(&[4], &[b"base"]))
                .unwrap();
        let canonical = dictionary
            .append_delta(1, &stream(&[100, 101], &[b"new", b"new"]))
            .unwrap();

        assert_eq!(canonical.ids, [5, 5]);
        assert_eq!(dictionary.deltas[0].entries.len(), 1);
    }

    #[test]
    fn sequential_appends_never_change_earlier_ids() {
        let mut dictionary =
            DuDictionaryLayersV0::from_base_stream("stable", &stream(&[8], &[b"base"])).unwrap();
        let first = dictionary
            .append_delta(1, &stream(&[0], &[b"first"]))
            .unwrap();
        let second = dictionary
            .append_delta(2, &stream(&[0], &[b"second"]))
            .unwrap();
        assert_eq!(first.ids, [9]);
        assert_eq!(second.ids, [10]);
        assert_eq!(dictionary.lookup_exact(b"base"), Some(8));
        assert_eq!(dictionary.lookup_exact(b"first"), Some(9));
        assert_eq!(dictionary.lookup_exact(b"second"), Some(10));
        assert_eq!(dictionary.lineage, "stable");
    }

    #[test]
    fn next_id_is_one_past_the_maximum_canonical_id() {
        let mut dictionary = DuDictionaryLayersV0::from_base_stream(
            "max-plus-one",
            &stream(&[41, 7], &[b"high", b"low"]),
        )
        .unwrap();
        assert_eq!(dictionary.next_id, 42);

        let canonical = dictionary
            .append_delta(1, &stream(&[0], &[b"new"]))
            .unwrap();

        assert_eq!(canonical.ids, [42]);
        assert_eq!(dictionary.next_id, 43);
    }

    #[test]
    fn derived_sorted_lookup_preserves_canonical_ids_and_layers() {
        let dictionary = DuDictionaryLayersV0::from_base_stream(
            "sorted-lookup",
            &stream(&[41, 42, 43], &[b"zebra", b"apple", b"river"]),
        )
        .unwrap();
        let before = dictionary.clone();

        let sorted = dictionary.derived_sorted_lookup().unwrap();

        assert_eq!(
            sorted
                .iter()
                .map(|entry| (entry.lexeme_bytes.as_slice(), entry.id))
                .collect::<Vec<_>>(),
            [
                (b"apple".as_slice(), 42),
                (b"river".as_slice(), 43),
                (b"zebra".as_slice(), 41),
            ]
        );
        assert_eq!(dictionary, before);
    }

    #[test]
    fn malformed_delta_layer_with_conflicting_id_fails_closed() {
        let mut duplicate = DuDictionaryLayersV0::from_base_stream(
            "malformed-duplicate",
            &stream(&[4], &[b"base"]),
        )
        .unwrap();
        duplicate.deltas.push(DictionaryDeltaLayerV0 {
            append_sequence: 1,
            entries: vec![
                DictionaryEntryV0 {
                    id: 5,
                    lexeme_bytes: b"first".to_vec(),
                },
                DictionaryEntryV0 {
                    id: 5,
                    lexeme_bytes: b"conflict".to_vec(),
                },
            ],
        });
        duplicate.next_id = 6;
        assert!(duplicate.validate().is_err());

        let mut conflicts_with_base = DuDictionaryLayersV0::from_base_stream(
            "malformed-base-conflict",
            &stream(&[4], &[b"base"]),
        )
        .unwrap();
        conflicts_with_base.deltas.push(DictionaryDeltaLayerV0 {
            append_sequence: 1,
            entries: vec![DictionaryEntryV0 {
                id: 4,
                lexeme_bytes: b"conflict".to_vec(),
            }],
        });
        conflicts_with_base.next_id = 5;
        assert!(conflicts_with_base.validate().is_err());
    }

    #[test]
    fn older_snapshot_remains_valid_after_dictionary_growth() {
        let older = snapshot();
        let older_bytes = older.to_json_vec().unwrap();
        let newer = append_bang(&older).updated_snapshot;

        assert_eq!(older.dictionary_layers.lookup_exact(b"!"), None);
        assert_eq!(newer.dictionary_layers.lookup_exact(b"!"), Some(9));
        assert_eq!(older.dictionary_layers.lookup_exact(b"abc"), Some(4));
        assert_eq!(newer.dictionary_layers.lookup_exact(b"abc"), Some(4));
        ProteusSnapshotV0::from_json_slice(&older_bytes).unwrap();
        older.validate().unwrap();
        newer.validate().unwrap();
    }

    #[test]
    fn public_dictionary_operations_preserve_assigned_ids() {
        let mut dictionary =
            DuDictionaryLayersV0::from_base_stream("append-only-api", &stream(&[4], &[b"base"]))
                .unwrap();
        dictionary
            .append_delta(1, &stream(&[0], &[b"delta"]))
            .unwrap();
        dictionary.validate().unwrap();
        let _ = dictionary.lookup_exact(b"delta");
        let _ = dictionary.derived_sorted_lookup().unwrap();

        assert_eq!(dictionary.lookup_exact(b"base"), Some(4));
        assert_eq!(dictionary.lookup_exact(b"delta"), Some(5));
    }

    #[test]
    fn snapshot_serde_round_trip_revalidates() {
        let snapshot = snapshot();
        let bytes = snapshot.to_json_vec().unwrap();
        let decoded = ProteusSnapshotV0::from_json_slice(&bytes).unwrap();
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn persisted_snapshot_load_preserves_all_assigned_ids() {
        let original = append_bang(&snapshot()).updated_snapshot;
        let path = std::env::temp_dir().join(format!(
            "proteus-round-trip-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        persist_snapshot(&path, &original).unwrap();
        let loaded = load_snapshot(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(loaded, original);
        assert_eq!(loaded.append_sequence, 1);
        assert_eq!(loaded.dictionary_layers.deltas[0].append_sequence, 1);
        assert_eq!(loaded.dictionary_layers.lookup_exact(b"abc"), Some(4));
        assert_eq!(loaded.dictionary_layers.lookup_exact(b"d"), Some(8));
        assert_eq!(loaded.dictionary_layers.lookup_exact(b"!"), Some(9));
    }

    #[test]
    fn snapshot_rejects_transcript_hash_or_length_mismatch() {
        let mut bad_len = snapshot();
        bad_len.transcript_len += 1;
        assert!(bad_len.validate().is_err());

        let mut bad_hash = snapshot();
        bad_hash.transcript_sha256 = "00".repeat(32);
        assert!(bad_hash.validate().is_err());
    }

    #[test]
    fn append_continues_exact_native_spans() {
        let artifact = append_bang(&snapshot());
        assert_eq!(artifact.candidate.ids, [10, 30, 31]);
        assert_eq!(
            artifact
                .candidate
                .native_spans
                .iter()
                .map(|span| (span.byte_start, span.byte_end))
                .collect::<Vec<_>>(),
            [(0, 2), (2, 3), (3, 5)]
        );
        assert_eq!(
            artifact
                .updated_snapshot
                .direct_id
                .du_state
                .reconstruct()
                .unwrap(),
            b"abcd!"
        );
        artifact.updated_snapshot.validate().unwrap();
    }

    #[test]
    fn candidate_is_constructed_without_an_oracle_api() {
        let artifact = append_bang(&snapshot());
        assert!(artifact.candidate.constructed_without_oracle_ids);
        assert_eq!(artifact.timings.oracle_full_tokenize_wall_ns, 0);
    }

    #[test]
    fn resume_total_excludes_oracle_timing() {
        let artifact = append_bang(&snapshot());
        let total_before = artifact.timings.proteus_resume_total_wall_ns;
        let expected = artifact.candidate.ids.clone();
        let mut oracle = |_bytes: &[u8]| -> Result<Vec<u32>> {
            thread::sleep(Duration::from_millis(2));
            Ok(expected.clone())
        };
        let verified = verify_with_full_tokenize_oracle(artifact, &mut oracle).unwrap();
        assert!(verified.ids_exact);
        assert_eq!(
            verified.artifact.timings.proteus_resume_total_wall_ns,
            total_before
        );
        assert!(verified.artifact.timings.oracle_full_tokenize_wall_ns > 0);
    }

    #[test]
    fn malformed_snapshot_and_dictionary_fail_closed() {
        let mut malformed = snapshot();
        malformed.dictionary_layers.next_id += 1;
        let json = serde_json::to_vec(&malformed).unwrap();
        assert!(ProteusSnapshotV0::from_json_slice(&json).is_err());

        let mut duplicate = snapshot();
        duplicate
            .dictionary_layers
            .base
            .entries
            .push(DictionaryEntryV0 {
                id: 4,
                lexeme_bytes: b"other".to_vec(),
            });
        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn malformed_delta_does_not_partially_mutate_dictionary() {
        let mut dictionary =
            DuDictionaryLayersV0::from_base_stream("transactional", &stream(&[4], &[b"base"]))
                .unwrap();
        let before = dictionary.clone();
        let malformed = stream(&[8, 9], &[b"new", b""]);

        assert!(dictionary.append_delta(1, &malformed).is_err());
        assert_eq!(dictionary, before);
    }

    #[test]
    fn invalid_serialized_snapshot_is_not_persisted() {
        let mut malformed = snapshot();
        malformed.dictionary_layers.next_id += 1;
        let bytes = serde_json::to_vec(&malformed).unwrap();
        let path = std::env::temp_dir().join(format!(
            "proteus-invalid-snapshot-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        assert!(persist_snapshot_bytes(&path, &bytes).is_err());
        assert!(!path.exists());
    }
}
