//! Exact tokenizer/model-I/O bridge primitives.
//!
//! This module intentionally deals only in token IDs, byte spans, and bytes.
//! It does not know how a tokenizer obtains its native IDs, which leaves
//! llama.cpp/Qwen/Mistral adapters as thin implementations of
//! [`NativeTokenLike`] later.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

/// A token emitted by a native tokenizer, including its exact source bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeToken {
    pub id: u64,
    pub byte_start: usize,
    pub byte_end: usize,
    pub bytes: Vec<u8>,
}

impl NativeToken {
    pub fn new(id: u64, byte_start: usize, byte_end: usize, bytes: Vec<u8>) -> Self {
        Self {
            id,
            byte_start,
            byte_end,
            bytes,
        }
    }
}

/// The JSONL representation accepted from external/native tokenizer tools.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeTokenJsonlRecord {
    pub id: u64,
    pub byte_start: usize,
    pub byte_end: usize,
    pub bytes_hex: String,
}

impl From<NativeToken> for NativeTokenJsonlRecord {
    fn from(token: NativeToken) -> Self {
        Self {
            id: token.id,
            byte_start: token.byte_start,
            byte_end: token.byte_end,
            bytes_hex: crate::bytes_to_hex(&token.bytes),
        }
    }
}

impl TryFrom<NativeTokenJsonlRecord> for NativeToken {
    type Error = anyhow::Error;

    fn try_from(record: NativeTokenJsonlRecord) -> Result<Self, Self::Error> {
        Ok(Self::new(
            record.id,
            record.byte_start,
            record.byte_end,
            crate::hex_to_bytes(&record.bytes_hex).context("decode native token bytes_hex")?,
        ))
    }
}

/// Trait implemented by an externally produced native token record.
///
/// Adapters can implement this for their own llama.cpp/Qwen/Mistral record
/// type without changing the bridge representation or alignment code.
pub trait NativeTokenLike {
    fn native_id(&self) -> u64;
    fn native_byte_start(&self) -> usize;
    fn native_byte_end(&self) -> usize;
    fn native_bytes(&self) -> &[u8];
}

impl NativeTokenLike for NativeToken {
    fn native_id(&self) -> u64 {
        self.id
    }

    fn native_byte_start(&self) -> usize {
        self.byte_start
    }

    fn native_byte_end(&self) -> usize {
        self.byte_end
    }

    fn native_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Generic adapter from an external native-record type to Nereids tokens.
pub struct NativeTokenizerAdapter;

impl NativeTokenizerAdapter {
    pub fn ingest<I, T>(records: I) -> Vec<NativeToken>
    where
        I: IntoIterator<Item = T>,
        T: NativeTokenLike,
    {
        records
            .into_iter()
            .map(|record| {
                NativeToken::new(
                    record.native_id(),
                    record.native_byte_start(),
                    record.native_byte_end(),
                    record.native_bytes().to_vec(),
                )
            })
            .collect()
    }

    pub fn ingest_jsonl_records<I>(records: I) -> Result<Vec<NativeToken>>
    where
        I: IntoIterator<Item = NativeTokenJsonlRecord>,
    {
        records.into_iter().map(NativeToken::try_from).collect()
    }
}

/// A tokenizer stream with provenance and the raw-input identity it covers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenizerStream {
    pub tokenizer_name: String,
    pub tokenizer_provenance: String,
    pub raw_sha256: String,
    pub raw_len: usize,
    pub tokens: Vec<NativeToken>,
}

impl TokenizerStream {
    pub fn new(
        tokenizer_name: impl Into<String>,
        tokenizer_provenance: impl Into<String>,
        raw: &[u8],
        tokens: Vec<NativeToken>,
    ) -> Result<Self> {
        let stream = Self {
            tokenizer_name: tokenizer_name.into(),
            tokenizer_provenance: tokenizer_provenance.into(),
            raw_sha256: crate::sha256_hex(raw),
            raw_len: raw.len(),
            tokens,
        };
        stream.validate(raw)?;
        Ok(stream)
    }

    /// Build a stream from externally supplied records. Validation is kept
    /// explicit so callers can report malformed streams with their own input
    /// context before attempting alignment.
    pub fn from_native_records<I, T>(
        tokenizer_name: impl Into<String>,
        tokenizer_provenance: impl Into<String>,
        raw: &[u8],
        records: I,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = T>,
        T: NativeTokenLike,
    {
        Self::new(
            tokenizer_name,
            tokenizer_provenance,
            raw,
            NativeTokenizerAdapter::ingest(records),
        )
    }

    /// Adapt the Phase 1 DU stream representation without changing it.
    pub fn from_loaded_stream(
        tokenizer_name: impl Into<String>,
        tokenizer_provenance: impl Into<String>,
        raw: &[u8],
        loaded: &crate::LoadedStream,
    ) -> Result<Self> {
        if loaded.ids.len() != loaded.parts.len() || loaded.bounds.len() != loaded.parts.len() + 1 {
            bail!("malformed loaded stream token/bound arrays");
        }
        let tokens = loaded
            .ids
            .iter()
            .zip(&loaded.parts)
            .enumerate()
            .map(|(index, (id, bytes))| {
                NativeToken::new(
                    *id,
                    loaded.bounds[index],
                    loaded.bounds[index + 1],
                    bytes.clone(),
                )
            })
            .collect();
        Self::new(tokenizer_name, tokenizer_provenance, raw, tokens)
    }

    /// Construct a stream without validating it. This is useful for import
    /// tools and tests that need to exercise gap/overlap diagnostics.
    pub fn unchecked(
        tokenizer_name: impl Into<String>,
        tokenizer_provenance: impl Into<String>,
        raw: &[u8],
        tokens: Vec<NativeToken>,
    ) -> Self {
        Self {
            tokenizer_name: tokenizer_name.into(),
            tokenizer_provenance: tokenizer_provenance.into(),
            raw_sha256: crate::sha256_hex(raw),
            raw_len: raw.len(),
            tokens,
        }
    }

    pub fn reconstruct(&self) -> Vec<u8> {
        self.tokens
            .iter()
            .flat_map(|token| token.bytes.iter().copied())
            .collect()
    }

    /// Validate metadata, contiguous spans, and each token's exact bytes.
    pub fn validate(&self, raw: &[u8]) -> Result<()> {
        if self.raw_len != raw.len() {
            bail!(
                "{} raw_len {} does not match input length {}",
                self.tokenizer_name,
                self.raw_len,
                raw.len()
            );
        }
        let expected_hash = crate::sha256_hex(raw);
        if self.raw_sha256 != expected_hash {
            bail!(
                "{} raw_sha256 {} does not match input hash {}",
                self.tokenizer_name,
                self.raw_sha256,
                expected_hash
            );
        }

        let mut expected_start = 0;
        for (index, token) in self.tokens.iter().enumerate() {
            if token.byte_start != expected_start {
                let kind = if token.byte_start < expected_start {
                    "overlap"
                } else {
                    "gap"
                };
                bail!(
                    "{} token {} has malformed {} at byte {} (expected {})",
                    self.tokenizer_name,
                    index,
                    kind,
                    token.byte_start,
                    expected_start
                );
            }
            if token.byte_end < token.byte_start {
                bail!(
                    "{} token {} has reversed span {}..{}",
                    self.tokenizer_name,
                    index,
                    token.byte_start,
                    token.byte_end
                );
            }
            if token.byte_start == token.byte_end {
                bail!(
                    "{} token {} has an empty span at byte {}",
                    self.tokenizer_name,
                    index,
                    token.byte_start
                );
            }
            if token.byte_end > raw.len() {
                bail!(
                    "{} token {} ends at {} beyond raw length {}",
                    self.tokenizer_name,
                    index,
                    token.byte_end,
                    raw.len()
                );
            }
            let expected = &raw[token.byte_start..token.byte_end];
            if token.bytes != expected {
                let mismatch = first_byte_mismatch(expected, &token.bytes);
                bail!(
                    "{} token {} bytes do not match raw span at relative byte {}",
                    self.tokenizer_name,
                    index,
                    mismatch.byte_offset
                );
            }
            expected_start = token.byte_end;
        }
        if expected_start != raw.len() {
            bail!(
                "{} stream has trailing gap: ends at {}, raw length {}",
                self.tokenizer_name,
                expected_start,
                raw.len()
            );
        }
        let reconstructed = self.reconstruct();
        if reconstructed != raw {
            bail!(
                "{} reconstruction mismatch at {:?}",
                self.tokenizer_name,
                crate::first_mismatch(&reconstructed, raw)
            );
        }
        Ok(())
    }

    pub fn import_jsonl<R: BufRead>(
        tokenizer_name: impl Into<String>,
        tokenizer_provenance: impl Into<String>,
        raw: &[u8],
        reader: R,
    ) -> Result<Self> {
        let mut records = Vec::new();
        for (line_no, line) in reader.lines().enumerate() {
            let line = line.with_context(|| format!("read native JSONL line {}", line_no + 1))?;
            if line.trim().is_empty() {
                continue;
            }
            let record: NativeTokenJsonlRecord = serde_json::from_str(&line)
                .with_context(|| format!("parse native JSONL line {}", line_no + 1))?;
            let token: NativeToken = record
                .try_into()
                .with_context(|| format!("decode native JSONL line {} bytes_hex", line_no + 1))?;
            records.push(token);
        }
        Self::from_native_records(tokenizer_name, tokenizer_provenance, raw, records)
    }

    pub fn export_jsonl<W: Write>(&self, mut writer: W) -> Result<usize> {
        for token in &self.tokens {
            let record: NativeTokenJsonlRecord = token.clone().into();
            serde_json::to_writer(&mut writer, &record)?;
            writer.write_all(b"\n")?;
        }
        Ok(self.tokens.len())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FirstMismatch {
    pub byte_offset: usize,
    pub expected_byte: Option<u8>,
    pub actual_byte: Option<u8>,
    pub expected_len: usize,
    pub actual_len: usize,
}

fn first_byte_mismatch(expected: &[u8], actual: &[u8]) -> FirstMismatch {
    let byte_offset = crate::first_mismatch(expected, actual).unwrap_or(expected.len());
    FirstMismatch {
        byte_offset,
        expected_byte: expected.get(byte_offset).copied(),
        actual_byte: actual.get(byte_offset).copied(),
        expected_len: expected.len(),
        actual_len: actual.len(),
    }
}

fn reconstruction_check(raw: &[u8], reconstructed: &[u8]) -> (bool, Option<FirstMismatch>) {
    let mismatch =
        crate::first_mismatch(raw, reconstructed).map(|_| first_byte_mismatch(raw, reconstructed));
    (mismatch.is_none(), mismatch)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenSpanAlignment {
    pub left_token_index: usize,
    pub left_token_id: u64,
    pub left_byte_start: usize,
    pub left_byte_end: usize,
    pub right_start_index: usize,
    pub right_end_index_exclusive: usize,
    pub right_token_count: usize,
    pub starts_on_right_boundary: bool,
    pub ends_on_right_boundary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundaryCrossStats {
    pub left_token_count: usize,
    pub right_token_count: usize,
    pub left_tokens_exactly_on_right_boundaries: usize,
    pub left_tokens_cross_or_cut_right_tokens: usize,
    pub left_tokens_start_inside_right_token: usize,
    pub left_tokens_end_inside_right_token: usize,
    pub max_right_tokens_covered_by_one_left_token: usize,
}

impl BoundaryCrossStats {
    fn from_alignments(
        left: &TokenizerStream,
        right: &TokenizerStream,
        alignments: &[TokenSpanAlignment],
    ) -> Self {
        let mut exact = 0;
        let mut cross_or_cut = 0;
        let mut start_inside = 0;
        let mut end_inside = 0;
        let mut max_covered = 0;
        for alignment in alignments {
            max_covered = max_covered.max(alignment.right_token_count);
            if alignment.starts_on_right_boundary && alignment.ends_on_right_boundary {
                exact += 1;
            } else {
                cross_or_cut += 1;
                if !alignment.starts_on_right_boundary {
                    start_inside += 1;
                }
                if !alignment.ends_on_right_boundary {
                    end_inside += 1;
                }
            }
        }
        Self {
            left_token_count: left.tokens.len(),
            right_token_count: right.tokens.len(),
            left_tokens_exactly_on_right_boundaries: exact,
            left_tokens_cross_or_cut_right_tokens: cross_or_cut,
            left_tokens_start_inside_right_token: start_inside,
            left_tokens_end_inside_right_token: end_inside,
            max_right_tokens_covered_by_one_left_token: max_covered,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenizerBridgeReport {
    pub mode: String,
    pub timestamp: String,
    pub nereids_version: String,
    pub input_bytes: usize,
    pub input_size: usize,
    pub input_sha256: String,
    pub source_tokenizer: String,
    pub source_provenance: String,
    pub source_token_count: usize,
    pub du_token_span_count: usize,
    pub target_tokenizer: String,
    pub target_provenance: String,
    pub target_token_count: usize,
    pub source_reconstruction_exact: bool,
    pub du_reconstruction_exact: bool,
    pub target_reconstruction_exact: bool,
    pub exact_reconstruction: bool,
    pub source_first_mismatch: Option<FirstMismatch>,
    pub du_first_mismatch: Option<FirstMismatch>,
    pub target_first_mismatch: Option<FirstMismatch>,
    pub first_mismatch_stream: Option<String>,
    pub first_mismatch: Option<FirstMismatch>,
    pub source_to_du: BoundaryCrossStats,
    pub du_to_target: BoundaryCrossStats,
    pub target_to_du: BoundaryCrossStats,
    pub wall_time_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenizerBridge {
    pub report: TokenizerBridgeReport,
    pub source_to_du: Vec<TokenSpanAlignment>,
    pub du_to_target: Vec<TokenSpanAlignment>,
    pub target_to_du: Vec<TokenSpanAlignment>,
}

fn stream_first_mismatch(raw: &[u8], stream: &TokenizerStream) -> Option<FirstMismatch> {
    reconstruction_check(raw, &stream.reconstruct()).1
}

fn first_mismatch_stream(
    raw: &[u8],
    source: &TokenizerStream,
    du: &TokenizerStream,
    target: &TokenizerStream,
) -> (Option<String>, Option<FirstMismatch>) {
    for (name, stream) in [("source", source), ("du", du), ("target", target)] {
        if let Some(mismatch) = stream_first_mismatch(raw, stream) {
            return (Some(name.to_string()), Some(mismatch));
        }
    }
    (None, None)
}

fn validate_pair(left: &TokenizerStream, right: &TokenizerStream) -> Result<()> {
    if left.raw_len != right.raw_len || left.raw_sha256 != right.raw_sha256 {
        bail!(
            "cannot align streams with different raw identity: {} vs {}",
            left.tokenizer_name,
            right.tokenizer_name
        );
    }
    Ok(())
}

/// Align every left token to all right tokens whose byte spans intersect it.
/// Both streams must have already been validated against the same raw bytes.
pub fn align_token_spans(
    left: &TokenizerStream,
    right: &TokenizerStream,
) -> Result<Vec<TokenSpanAlignment>> {
    validate_pair(left, right)?;

    /*
     * Both streams are validated as ordered, contiguous partitions of the
     * same raw byte stream. Their cursors therefore only move forward.
     *
     * This is O(left_tokens + right_tokens), rather than rescanning or doing
     * an independent binary search of the right stream for each left token.
     */
    let mut output = Vec::with_capacity(left.tokens.len());
    let mut right_start = 0usize;
    let mut right_end = 0usize;

    for (left_index, left_token) in left.tokens.iter().enumerate() {
        let start = left_token.byte_start;
        let end = left_token.byte_end;

        while right_start < right.tokens.len() && right.tokens[right_start].byte_end <= start {
            right_start += 1;
        }

        if right_end < right_start {
            right_end = right_start;
        }

        while right_end < right.tokens.len() && right.tokens[right_end].byte_start < end {
            right_end += 1;
        }

        let starts_on_boundary =
            right_start < right.tokens.len() && right.tokens[right_start].byte_start == start;

        let ends_on_boundary = if right_end < right.tokens.len() {
            right.tokens[right_end].byte_start == end
        } else {
            end == right.raw_len
        };

        output.push(TokenSpanAlignment {
            left_token_index: left_index,
            left_token_id: left_token.id,
            left_byte_start: start,
            left_byte_end: end,
            right_start_index: right_start,
            right_end_index_exclusive: right_end,
            right_token_count: right_end.saturating_sub(right_start),
            starts_on_right_boundary: starts_on_boundary,
            ends_on_right_boundary: ends_on_boundary,
        });
    }

    Ok(output)
}

/// Validate all three streams, create source→DU, DU→target, and target→DU
/// alignments, then report exact reconstruction and boundary statistics.
pub fn bridge_tokenizer_streams(
    raw: &[u8],
    source: &TokenizerStream,
    du: &TokenizerStream,
    target: &TokenizerStream,
) -> Result<TokenizerBridge> {
    bridge_tokenizer_streams_timed(raw, source, du, target, None)
}

pub fn bridge_tokenizer_streams_timed(
    raw: &[u8],
    source: &TokenizerStream,
    du: &TokenizerStream,
    target: &TokenizerStream,
    wall_time_ms: Option<u128>,
) -> Result<TokenizerBridge> {
    source
        .validate(raw)
        .context("validate source tokenizer stream")?;
    du.validate(raw).context("validate DU tokenizer stream")?;
    target
        .validate(raw)
        .context("validate target tokenizer stream")?;

    let source_to_du = align_token_spans(source, du)?;
    let du_to_target = align_token_spans(du, target)?;
    let target_to_du = align_token_spans(target, du)?;
    let (source_exact, source_mismatch) = reconstruction_check(raw, &source.reconstruct());
    let (du_exact, du_mismatch) = reconstruction_check(raw, &du.reconstruct());
    let (target_exact, target_mismatch) = reconstruction_check(raw, &target.reconstruct());
    let (mismatch_stream, mismatch) = first_mismatch_stream(raw, source, du, target);

    let report = TokenizerBridgeReport {
        mode: "exact tokenizer/model-I/O bridge".to_string(),
        timestamp: crate::timestamp(),
        nereids_version: crate::version().to_string(),
        input_bytes: raw.len(),
        input_size: raw.len(),
        input_sha256: crate::sha256_hex(raw),
        source_tokenizer: source.tokenizer_name.clone(),
        source_provenance: source.tokenizer_provenance.clone(),
        source_token_count: source.tokens.len(),
        du_token_span_count: du.tokens.len(),
        target_tokenizer: target.tokenizer_name.clone(),
        target_provenance: target.tokenizer_provenance.clone(),
        target_token_count: target.tokens.len(),
        source_reconstruction_exact: source_exact,
        du_reconstruction_exact: du_exact,
        target_reconstruction_exact: target_exact,
        exact_reconstruction: source_exact && du_exact && target_exact,
        source_first_mismatch: source_mismatch,
        du_first_mismatch: du_mismatch,
        target_first_mismatch: target_mismatch,
        first_mismatch_stream: mismatch_stream,
        first_mismatch: mismatch,
        source_to_du: BoundaryCrossStats::from_alignments(source, du, &source_to_du),
        du_to_target: BoundaryCrossStats::from_alignments(du, target, &du_to_target),
        target_to_du: BoundaryCrossStats::from_alignments(target, du, &target_to_du),
        wall_time_ms,
    };

    Ok(TokenizerBridge {
        report,
        source_to_du,
        du_to_target,
        target_to_du,
    })
}

/// Import one JSONL token file and attach the raw identity supplied by the
/// caller. Each non-empty line is one `NativeTokenJsonlRecord`.
pub fn import_native_token_jsonl(
    path: impl AsRef<std::path::Path>,
    tokenizer_name: impl Into<String>,
    tokenizer_provenance: impl Into<String>,
    raw: &[u8],
) -> Result<TokenizerStream> {
    let path = path.as_ref();
    let file = std::fs::File::open(path)
        .with_context(|| format!("open native token JSONL {}", path.display()))?;
    TokenizerStream::import_jsonl(
        tokenizer_name,
        tokenizer_provenance,
        raw,
        std::io::BufReader::new(file),
    )
}

/// Export the token records (metadata remains an application-level stream
/// concern, so JSONL stays compatible with simple tokenizer tooling).
pub fn export_native_token_jsonl(
    stream: &TokenizerStream,
    path: impl AsRef<std::path::Path>,
) -> Result<usize> {
    let path = path.as_ref();
    let file = std::fs::File::create(path)
        .with_context(|| format!("create native token JSONL {}", path.display()))?;
    stream.export_jsonl(std::io::BufWriter::new(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(id: u64, start: usize, end: usize, bytes: &[u8]) -> NativeToken {
        NativeToken::new(id, start, end, bytes.to_vec())
    }

    fn stream(name: &str, raw: &[u8], tokens: Vec<NativeToken>) -> TokenizerStream {
        TokenizerStream::new(name, format!("test:{name}"), raw, tokens).unwrap()
    }

    fn partition_stream(name: &str, raw: &[u8], mask: usize) -> TokenizerStream {
        let mut tokens = Vec::new();
        let mut start = 0usize;
        let mut id = 1u64;

        for boundary in 1..raw.len() {
            if mask & (1usize << (boundary - 1)) != 0 {
                tokens.push(token(id, start, boundary, &raw[start..boundary]));
                id += 1;
                start = boundary;
            }
        }

        tokens.push(token(id, start, raw.len(), &raw[start..]));
        stream(name, raw, tokens)
    }

    fn reference_align(left: &TokenizerStream, right: &TokenizerStream) -> Vec<TokenSpanAlignment> {
        let right_bounds: Vec<usize> = right
            .tokens
            .iter()
            .map(|token| token.byte_start)
            .chain(right.tokens.last().map(|token| token.byte_end))
            .collect();

        left.tokens
            .iter()
            .enumerate()
            .map(|(left_index, left_token)| {
                let start = left_token.byte_start;
                let end = left_token.byte_end;

                let right_start = right
                    .tokens
                    .iter()
                    .position(|token| token.byte_end > start)
                    .unwrap_or(right.tokens.len());

                let right_end = right
                    .tokens
                    .iter()
                    .position(|token| token.byte_start >= end)
                    .unwrap_or(right.tokens.len());

                TokenSpanAlignment {
                    left_token_index: left_index,
                    left_token_id: left_token.id,
                    left_byte_start: start,
                    left_byte_end: end,
                    right_start_index: right_start,
                    right_end_index_exclusive: right_end,
                    right_token_count: right_end.saturating_sub(right_start),
                    starts_on_right_boundary: right_bounds.binary_search(&start).is_ok(),
                    ends_on_right_boundary: right_bounds.binary_search(&end).is_ok(),
                }
            })
            .collect()
    }

    #[test]
    fn linear_alignment_matches_reference_for_all_small_partitions() {
        let raw = b"abcdefgh";
        let partitions = 1usize << (raw.len() - 1);

        for left_mask in 0..partitions {
            let left = partition_stream("left", raw, left_mask);

            for right_mask in 0..partitions {
                let right = partition_stream("right", raw, right_mask);

                let expected = reference_align(&left, &right);
                let actual = align_token_spans(&left, &right).unwrap();

                assert_eq!(
                    actual, expected,
                    "partition mismatch left_mask={left_mask:#09b} \
                     right_mask={right_mask:#09b}"
                );
            }
        }
    }

    #[test]
    fn exact_contiguous_token_stream() {
        let raw = b"hello";
        let stream = stream("source", raw, vec![token(123, 0, 5, b"hello")]);
        assert_eq!(stream.reconstruct(), raw);
        stream.validate(raw).unwrap();
    }

    #[test]
    fn malformed_gap_is_rejected() {
        let stream = TokenizerStream::unchecked(
            "source",
            "test",
            b"hello",
            vec![token(1, 0, 2, b"he"), token(2, 3, 5, b"lo")],
        );
        let error = stream.validate(b"hello").unwrap_err().to_string();
        assert!(error.contains("gap"));
    }

    #[test]
    fn malformed_overlap_is_rejected() {
        let stream = TokenizerStream::unchecked(
            "source",
            "test",
            b"hello",
            vec![token(1, 0, 3, b"hel"), token(2, 2, 5, b"llo")],
        );
        let error = stream.validate(b"hello").unwrap_err().to_string();
        assert!(error.contains("overlap"));
    }

    #[test]
    fn incorrect_token_bytes_are_rejected() {
        let stream =
            TokenizerStream::unchecked("source", "test", b"hello", vec![token(1, 0, 5, b"hullo")]);
        let error = stream.validate(b"hello").unwrap_err().to_string();
        assert!(error.contains("bytes do not match"));
    }

    #[test]
    fn multibyte_utf8_is_aligned_by_bytes() {
        let raw = "hé界".as_bytes();
        let source = stream(
            "source",
            raw,
            vec![
                token(1, 0, 3, "hé".as_bytes()),
                token(6, 3, 6, "界".as_bytes()),
            ],
        );
        let du = stream(
            "du",
            raw,
            vec![
                token(2, 0, 1, "h".as_bytes()),
                token(3, 1, 3, "é".as_bytes()),
                token(7, 3, 6, "界".as_bytes()),
            ],
        );
        let target = stream(
            "target",
            raw,
            vec![
                token(4, 0, 3, "hé".as_bytes()),
                token(5, 3, 6, "界".as_bytes()),
            ],
        );
        let bridge = bridge_tokenizer_streams(raw, &source, &du, &target).unwrap();
        assert!(bridge.report.exact_reconstruction);
        assert_eq!(bridge.source_to_du[0].right_token_count, 2);
        assert_eq!(bridge.target_to_du[0].right_token_count, 2);
    }

    #[test]
    fn different_boundaries_preserve_exact_bytes() {
        let raw = b"abcdef";
        let source = stream(
            "source",
            raw,
            vec![token(10, 0, 2, b"ab"), token(11, 2, 6, b"cdef")],
        );
        let du = stream(
            "du",
            raw,
            vec![token(20, 0, 3, b"abc"), token(21, 3, 6, b"def")],
        );
        let target = stream(
            "target",
            raw,
            vec![
                token(30, 0, 1, b"a"),
                token(31, 1, 4, b"bcd"),
                token(32, 4, 6, b"ef"),
            ],
        );
        let bridge = bridge_tokenizer_streams(raw, &source, &du, &target).unwrap();
        assert!(bridge.report.source_reconstruction_exact);
        assert!(bridge.report.du_reconstruction_exact);
        assert!(bridge.report.target_reconstruction_exact);
        assert_eq!(
            bridge
                .report
                .source_to_du
                .left_tokens_cross_or_cut_right_tokens,
            2
        );
        assert_eq!(
            bridge
                .report
                .target_to_du
                .max_right_tokens_covered_by_one_left_token,
            2
        );
    }

    #[test]
    fn target_token_crosses_multiple_du_spans() {
        let raw = b"abc";
        let source = stream("source", raw, vec![token(1, 0, 3, raw)]);
        let du = stream(
            "du",
            raw,
            vec![
                token(2, 0, 1, b"a"),
                token(3, 1, 2, b"b"),
                token(4, 2, 3, b"c"),
            ],
        );
        let target = stream("target", raw, vec![token(5, 0, 3, raw)]);
        let bridge = bridge_tokenizer_streams(raw, &source, &du, &target).unwrap();
        assert_eq!(bridge.target_to_du[0].right_token_count, 3);
        assert_eq!(
            bridge
                .report
                .target_to_du
                .max_right_tokens_covered_by_one_left_token,
            3
        );
    }

    #[test]
    fn jsonl_round_trip_uses_bytes_hex() {
        let raw = b"hello";
        let stream = stream("source", raw, vec![token(123, 0, 5, raw)]);
        let mut jsonl = Vec::new();
        stream.export_jsonl(&mut jsonl).unwrap();
        assert_eq!(
            std::str::from_utf8(&jsonl).unwrap(),
            "{\"id\":123,\"byte_start\":0,\"byte_end\":5,\"bytes_hex\":\"68656c6c6f\"}\n"
        );
        let imported = TokenizerStream::import_jsonl(
            "source",
            "test:source",
            raw,
            std::io::Cursor::new(jsonl),
        )
        .unwrap();
        assert_eq!(imported.tokens, stream.tokens);
    }

    #[test]
    fn generic_native_adapter_preserves_external_ids() {
        #[derive(Clone)]
        struct External {
            id: u64,
            start: usize,
            end: usize,
            bytes: Vec<u8>,
        }
        impl NativeTokenLike for External {
            fn native_id(&self) -> u64 {
                self.id
            }
            fn native_byte_start(&self) -> usize {
                self.start
            }
            fn native_byte_end(&self) -> usize {
                self.end
            }
            fn native_bytes(&self) -> &[u8] {
                &self.bytes
            }
        }
        let raw = b"hello";
        let stream = TokenizerStream::from_native_records(
            "external",
            "test:external",
            raw,
            [External {
                id: 987,
                start: 0,
                end: 5,
                bytes: raw.to_vec(),
            }],
        )
        .unwrap();
        assert_eq!(stream.tokens[0].id, 987);
    }
}
