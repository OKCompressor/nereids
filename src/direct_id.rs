//! Bounded append repair for canonical native tokenizer IDs.
//!
//! The V1 API detokenizes kept native IDs to discover a repair boundary. The
//! V2 API instead reads that boundary from persisted [`crate::TokenSpan`]
//! records and reconstructs the snapshot tail and appended bytes from
//! [`crate::LoadedStream`] DU state. Both APIs expose only repair-window
//! tokenization during candidate construction. Full tokenization belongs to
//! the separate verification oracle used by callers.

use serde::Serialize;
use std::error::Error;
use std::fmt;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateError {
    DropExceedsPrefix {
        drop_tokens: usize,
        prefix_token_count: usize,
    },
    Detokenize(String),
    KeptBytesNotPrefix {
        drop_tokens: usize,
        kept_byte_len: usize,
    },
    FullWindowForbidden {
        drop_tokens: usize,
    },
    Tokenize(String),
}

impl CandidateError {
    fn is_callback_error(&self) -> bool {
        matches!(self, Self::Detokenize(_) | Self::Tokenize(_))
    }
}

impl fmt::Display for CandidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DropExceedsPrefix {
                drop_tokens,
                prefix_token_count,
            } => write!(
                f,
                "cannot drop {drop_tokens} tokens from a {prefix_token_count}-token snapshot"
            ),
            Self::Detokenize(error) => write!(f, "native detokenize failed: {error}"),
            Self::KeptBytesNotPrefix {
                drop_tokens,
                kept_byte_len,
            } => write!(
                f,
                "detokenized bytes for drop={drop_tokens} (length {kept_byte_len}) are not an exact prefix of the snapshot bytes"
            ),
            Self::FullWindowForbidden { drop_tokens } => write!(
                f,
                "drop={drop_tokens} would tokenize the complete prefix plus delta"
            ),
            Self::Tokenize(error) => write!(f, "native repair-window tokenize failed: {error}"),
        }
    }
}

impl Error for CandidateError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairCandidate {
    pub ids: Vec<u32>,
    pub drop_tokens: usize,
    pub kept_snapshot_token_count: usize,
    pub kept_byte_len: usize,
    pub snapshot_tail_bytes_repaired: usize,
    pub repair_window_bytes: usize,
    pub repair_token_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairTrial {
    pub drop_tokens: usize,
    pub kept_snapshot_token_count: Option<usize>,
    pub kept_byte_len: Option<usize>,
    pub snapshot_tail_bytes_repaired: Option<usize>,
    pub repair_window_bytes: Option<usize>,
    pub repair_token_count: Option<usize>,
    pub candidate_token_count: Option<usize>,
    pub ids_exact: bool,
    pub rejected: Option<String>,
}

impl RepairTrial {
    fn from_candidate(candidate: &RepairCandidate, ids_exact: bool) -> Self {
        Self {
            drop_tokens: candidate.drop_tokens,
            kept_snapshot_token_count: Some(candidate.kept_snapshot_token_count),
            kept_byte_len: Some(candidate.kept_byte_len),
            snapshot_tail_bytes_repaired: Some(candidate.snapshot_tail_bytes_repaired),
            repair_window_bytes: Some(candidate.repair_window_bytes),
            repair_token_count: Some(candidate.repair_token_count),
            candidate_token_count: Some(candidate.ids.len()),
            ids_exact,
            rejected: None,
        }
    }

    fn rejected(drop_tokens: usize, error: &CandidateError) -> Self {
        Self {
            drop_tokens,
            kept_snapshot_token_count: None,
            kept_byte_len: None,
            snapshot_tail_bytes_repaired: None,
            repair_window_bytes: None,
            repair_token_count: None,
            candidate_token_count: None,
            ids_exact: false,
            rejected: Some(error.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactRepair {
    pub candidate: RepairCandidate,
    pub trials: Vec<RepairTrial>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairSearchError {
    CandidateCallback(CandidateError),
    Exhausted {
        max_drop_tokens: usize,
        trials: Vec<RepairTrial>,
    },
}

impl fmt::Display for RepairSearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CandidateCallback(error) => error.fmt(f),
            Self::Exhausted {
                max_drop_tokens, ..
            } => write!(
                f,
                "no exact Direct-ID repair candidate found for drop_tokens=0..={max_drop_tokens}"
            ),
        }
    }
}

impl Error for RepairSearchError {}

/// Construct one append-repair candidate without full-tokenizing
/// `prefix || delta`.
pub fn construct_repair_candidate<Tokenize, Detokenize>(
    prefix: &[u8],
    delta: &[u8],
    prefix_ids: &[u32],
    drop_tokens: usize,
    tokenize_repair_window: &mut Tokenize,
    detokenize_kept_ids: &mut Detokenize,
) -> Result<RepairCandidate, CandidateError>
where
    Tokenize: FnMut(&[u8]) -> Result<Vec<u32>, String>,
    Detokenize: FnMut(&[u32]) -> Result<Vec<u8>, String>,
{
    let kept_snapshot_token_count =
        prefix_ids
            .len()
            .checked_sub(drop_tokens)
            .ok_or(CandidateError::DropExceedsPrefix {
                drop_tokens,
                prefix_token_count: prefix_ids.len(),
            })?;
    let kept_ids = &prefix_ids[..kept_snapshot_token_count];
    let kept_bytes = detokenize_kept_ids(kept_ids).map_err(CandidateError::Detokenize)?;

    if !prefix.starts_with(&kept_bytes) {
        return Err(CandidateError::KeptBytesNotPrefix {
            drop_tokens,
            kept_byte_len: kept_bytes.len(),
        });
    }

    // The snapshot must retain some bytes. Otherwise this candidate would
    // silently become the prohibited full-window fallback.
    if kept_bytes.is_empty() {
        return Err(CandidateError::FullWindowForbidden { drop_tokens });
    }

    let snapshot_tail = &prefix[kept_bytes.len()..];
    let mut repair_window = Vec::with_capacity(snapshot_tail.len() + delta.len());
    repair_window.extend_from_slice(snapshot_tail);
    repair_window.extend_from_slice(delta);

    let repair_ids = tokenize_repair_window(&repair_window).map_err(CandidateError::Tokenize)?;
    let mut ids = Vec::with_capacity(kept_ids.len() + repair_ids.len());
    ids.extend_from_slice(kept_ids);
    ids.extend_from_slice(&repair_ids);

    Ok(RepairCandidate {
        ids,
        drop_tokens,
        kept_snapshot_token_count,
        kept_byte_len: kept_bytes.len(),
        snapshot_tail_bytes_repaired: snapshot_tail.len(),
        repair_window_bytes: repair_window.len(),
        repair_token_count: repair_ids.len(),
    })
}

/// Search the configured inclusive drop range and return the first candidate
/// whose IDs equal the separately computed verification oracle.
pub fn search_exact_repair<Tokenize, Detokenize>(
    prefix: &[u8],
    delta: &[u8],
    prefix_ids: &[u32],
    oracle_ids: &[u32],
    max_drop_tokens: usize,
    tokenize_repair_window: &mut Tokenize,
    detokenize_kept_ids: &mut Detokenize,
) -> Result<ExactRepair, RepairSearchError>
where
    Tokenize: FnMut(&[u8]) -> Result<Vec<u32>, String>,
    Detokenize: FnMut(&[u32]) -> Result<Vec<u8>, String>,
{
    let mut trials = Vec::new();

    for drop_tokens in 0..=max_drop_tokens {
        match construct_repair_candidate(
            prefix,
            delta,
            prefix_ids,
            drop_tokens,
            tokenize_repair_window,
            detokenize_kept_ids,
        ) {
            Ok(candidate) => {
                let ids_exact = candidate.ids == oracle_ids;
                trials.push(RepairTrial::from_candidate(&candidate, ids_exact));
                if ids_exact {
                    return Ok(ExactRepair { candidate, trials });
                }
            }
            Err(error) if error.is_callback_error() => {
                return Err(RepairSearchError::CandidateCallback(error));
            }
            Err(error) => trials.push(RepairTrial::rejected(drop_tokens, &error)),
        }
    }

    Err(RepairSearchError::Exhausted {
        max_drop_tokens,
        trials,
    })
}

/// Persisted Direct-ID snapshot with exact native token spans and the DU
/// structural state that reconstructs the snapshot bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DirectIdSnapshotV2 {
    pub prefix_len: usize,
    pub prefix_sha256: String,
    pub native_ids: Vec<u32>,
    pub native_spans: Vec<crate::TokenSpan>,
    pub du_state: crate::LoadedStream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectIdSnapshotError {
    message: String,
}

impl DirectIdSnapshotError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DirectIdSnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for DirectIdSnapshotError {}

impl DirectIdSnapshotV2 {
    pub fn new(
        prefix: &[u8],
        native_ids: Vec<u32>,
        native_spans: Vec<crate::TokenSpan>,
        du_state: crate::LoadedStream,
    ) -> Result<Self, DirectIdSnapshotError> {
        let snapshot = Self {
            prefix_len: prefix.len(),
            prefix_sha256: crate::sha256_hex(prefix),
            native_ids,
            native_spans,
            du_state,
        };
        snapshot.validate_against(prefix)?;
        Ok(snapshot)
    }

    /// Capture an already validated native tokenizer stream without defining
    /// another native span representation.
    pub fn from_tokenizer_stream(
        prefix: &[u8],
        native_stream: &crate::TokenizerStream,
        du_state: crate::LoadedStream,
    ) -> Result<Self, DirectIdSnapshotError> {
        native_stream
            .validate(prefix)
            .map_err(|error| DirectIdSnapshotError::new(error.to_string()))?;

        let mut native_ids = Vec::with_capacity(native_stream.tokens.len());
        let mut native_spans = Vec::with_capacity(native_stream.tokens.len());
        for token in &native_stream.tokens {
            let id = u32::try_from(token.id).map_err(|_| {
                DirectIdSnapshotError::new(format!(
                    "native token ID {} cannot be represented as u32",
                    token.id
                ))
            })?;
            native_ids.push(id);
            native_spans.push(crate::TokenSpan {
                id: token.id,
                byte_start: token.byte_start,
                byte_end: token.byte_end,
                bytes_hex: crate::bytes_to_hex(&token.bytes),
            });
        }

        Self::new(prefix, native_ids, native_spans, du_state)
    }

    /// Validate all persisted snapshot invariants. This is snapshot-load
    /// preflight, not candidate construction or candidate timing.
    pub fn validate(&self) -> Result<(), DirectIdSnapshotError> {
        let prefix = self
            .du_state
            .reconstruct()
            .map_err(|error| DirectIdSnapshotError::new(error.to_string()))?;
        self.validate_against(&prefix)
    }

    pub fn validate_against(&self, prefix: &[u8]) -> Result<(), DirectIdSnapshotError> {
        if self.prefix_len == 0 {
            return Err(DirectIdSnapshotError::new(
                "Direct-ID V2 snapshots must retain a nonempty prefix",
            ));
        }
        if prefix.len() != self.prefix_len {
            return Err(DirectIdSnapshotError::new(format!(
                "snapshot prefix length {} does not match reconstructed length {}",
                self.prefix_len,
                prefix.len()
            )));
        }
        let prefix_hash = crate::sha256_hex(prefix);
        if prefix_hash != self.prefix_sha256 {
            return Err(DirectIdSnapshotError::new(format!(
                "snapshot prefix hash {} does not match reconstructed hash {}",
                self.prefix_sha256, prefix_hash
            )));
        }
        let du_prefix = self
            .du_state
            .reconstruct()
            .map_err(|error| DirectIdSnapshotError::new(error.to_string()))?;
        if du_prefix != prefix {
            return Err(DirectIdSnapshotError::new(
                "snapshot DU state does not reconstruct the exact prefix bytes",
            ));
        }
        if self.native_ids.len() != self.native_spans.len() {
            return Err(DirectIdSnapshotError::new(format!(
                "snapshot has {} native IDs but {} native spans",
                self.native_ids.len(),
                self.native_spans.len()
            )));
        }
        if self.native_spans.is_empty() {
            return Err(DirectIdSnapshotError::new(
                "nonempty snapshot has no native token spans",
            ));
        }

        let mut expected_start = 0usize;
        for (index, (native_id, span)) in self.native_ids.iter().zip(&self.native_spans).enumerate()
        {
            if span.id != u64::from(*native_id) {
                return Err(DirectIdSnapshotError::new(format!(
                    "native span {} ID {} does not match stored native ID {}",
                    index, span.id, native_id
                )));
            }
            if span.byte_start != expected_start {
                return Err(DirectIdSnapshotError::new(format!(
                    "native span {} starts at {}, expected {}",
                    index, span.byte_start, expected_start
                )));
            }
            if span.byte_end <= span.byte_start {
                return Err(DirectIdSnapshotError::new(format!(
                    "native span {} is empty or reversed: {}..{}",
                    index, span.byte_start, span.byte_end
                )));
            }
            if span.byte_end > prefix.len() {
                return Err(DirectIdSnapshotError::new(format!(
                    "native span {} ends at {} beyond prefix length {}",
                    index,
                    span.byte_end,
                    prefix.len()
                )));
            }
            let span_bytes = crate::hex_to_bytes(&span.bytes_hex).map_err(|error| {
                DirectIdSnapshotError::new(format!(
                    "native span {} has invalid bytes_hex: {}",
                    index, error
                ))
            })?;
            if span_bytes != prefix[span.byte_start..span.byte_end] {
                return Err(DirectIdSnapshotError::new(format!(
                    "native span {} bytes do not match prefix range {}..{}",
                    index, span.byte_start, span.byte_end
                )));
            }
            expected_start = span.byte_end;
        }
        if expected_start != self.prefix_len {
            return Err(DirectIdSnapshotError::new(format!(
                "native spans end at {}, expected prefix length {}",
                expected_start, self.prefix_len
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuAwareCandidateError {
    InvalidSnapshot(DirectIdSnapshotError),
    InvalidDelta(String),
    DropExceedsPrefix {
        drop_tokens: usize,
        prefix_token_count: usize,
    },
    FullWindowForbidden {
        drop_tokens: usize,
    },
    RepairStartOutsidePrefix {
        repair_start_byte: usize,
        prefix_len: usize,
    },
    SnapshotTailReconstruction(String),
    Tokenize(String),
}

impl DuAwareCandidateError {
    fn is_callback_error(&self) -> bool {
        matches!(self, Self::Tokenize(_))
    }
}

impl fmt::Display for DuAwareCandidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSnapshot(error) => write!(f, "invalid Direct-ID V2 snapshot: {error}"),
            Self::InvalidDelta(error) => write!(f, "invalid DU delta stream: {error}"),
            Self::DropExceedsPrefix {
                drop_tokens,
                prefix_token_count,
            } => write!(
                f,
                "cannot drop {drop_tokens} tokens from a {prefix_token_count}-token snapshot"
            ),
            Self::FullWindowForbidden { drop_tokens } => write!(
                f,
                "drop={drop_tokens} would tokenize the complete prefix plus delta"
            ),
            Self::RepairStartOutsidePrefix {
                repair_start_byte,
                prefix_len,
            } => write!(
                f,
                "stored repair start {repair_start_byte} is outside prefix length {prefix_len}"
            ),
            Self::SnapshotTailReconstruction(error) => {
                write!(f, "snapshot DU tail reconstruction failed: {error}")
            }
            Self::Tokenize(error) => write!(f, "native repair-window tokenize failed: {error}"),
        }
    }
}

impl Error for DuAwareCandidateError {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DuAwareRepairTimings {
    pub du_reconstruct_wall_ns: u128,
    pub span_lookup_wall_ns: u128,
    pub repair_tokenize_wall_ns: u128,
    pub splice_wall_ns: u128,
    pub candidate_total_prep_wall_ns: u128,
}

impl DuAwareRepairTimings {
    fn add_assign(&mut self, other: &Self) {
        self.du_reconstruct_wall_ns = self
            .du_reconstruct_wall_ns
            .saturating_add(other.du_reconstruct_wall_ns);
        self.span_lookup_wall_ns = self
            .span_lookup_wall_ns
            .saturating_add(other.span_lookup_wall_ns);
        self.repair_tokenize_wall_ns = self
            .repair_tokenize_wall_ns
            .saturating_add(other.repair_tokenize_wall_ns);
        self.splice_wall_ns = self.splice_wall_ns.saturating_add(other.splice_wall_ns);
        self.candidate_total_prep_wall_ns = self
            .candidate_total_prep_wall_ns
            .saturating_add(other.candidate_total_prep_wall_ns);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuAwareRepairCandidate {
    pub ids: Vec<u32>,
    pub drop_tokens: usize,
    pub kept_snapshot_token_count: usize,
    pub repair_start_byte: usize,
    pub snapshot_tail_bytes: usize,
    pub repair_window_bytes: usize,
    pub repair_token_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuAwareRepairTrial {
    pub drop_tokens: usize,
    pub kept_snapshot_token_count: Option<usize>,
    pub repair_start_byte: Option<usize>,
    pub snapshot_tail_bytes: Option<usize>,
    pub repair_window_bytes: Option<usize>,
    pub repair_token_count: Option<usize>,
    pub candidate_token_count: Option<usize>,
    pub ids_exact: bool,
    pub rejected: Option<String>,
}

impl DuAwareRepairTrial {
    fn from_candidate(candidate: &DuAwareRepairCandidate, ids_exact: bool) -> Self {
        Self {
            drop_tokens: candidate.drop_tokens,
            kept_snapshot_token_count: Some(candidate.kept_snapshot_token_count),
            repair_start_byte: Some(candidate.repair_start_byte),
            snapshot_tail_bytes: Some(candidate.snapshot_tail_bytes),
            repair_window_bytes: Some(candidate.repair_window_bytes),
            repair_token_count: Some(candidate.repair_token_count),
            candidate_token_count: Some(candidate.ids.len()),
            ids_exact,
            rejected: None,
        }
    }

    fn rejected(drop_tokens: usize, error: &DuAwareCandidateError) -> Self {
        Self {
            drop_tokens,
            kept_snapshot_token_count: None,
            repair_start_byte: None,
            snapshot_tail_bytes: None,
            repair_window_bytes: None,
            repair_token_count: None,
            candidate_token_count: None,
            ids_exact: false,
            rejected: Some(error.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuAwareExactRepair {
    pub candidate: DuAwareRepairCandidate,
    pub reconstructed_delta: Vec<u8>,
    pub timings: DuAwareRepairTimings,
    pub trials: Vec<DuAwareRepairTrial>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuAwareRepairSearchError {
    Candidate(DuAwareCandidateError),
    Exhausted {
        max_drop_tokens: usize,
        timings: DuAwareRepairTimings,
        trials: Vec<DuAwareRepairTrial>,
    },
}

impl fmt::Display for DuAwareRepairSearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Candidate(error) => error.fmt(f),
            Self::Exhausted {
                max_drop_tokens, ..
            } => write!(
                f,
                "no exact DU-aware Direct-ID repair candidate found for drop_tokens=0..={max_drop_tokens}"
            ),
        }
    }
}

impl Error for DuAwareRepairSearchError {}

fn construct_du_aware_candidate_from_delta<Tokenize>(
    snapshot: &DirectIdSnapshotV2,
    delta_bytes: &[u8],
    drop_tokens: usize,
    tokenize_repair_window: &mut Tokenize,
) -> Result<(DuAwareRepairCandidate, DuAwareRepairTimings), DuAwareCandidateError>
where
    Tokenize: FnMut(&[u8]) -> Result<Vec<u32>, String>,
{
    let total_started = Instant::now();
    let mut timings = DuAwareRepairTimings::default();

    let span_started = Instant::now();
    let kept_snapshot_token_count = snapshot.native_ids.len().checked_sub(drop_tokens).ok_or(
        DuAwareCandidateError::DropExceedsPrefix {
            drop_tokens,
            prefix_token_count: snapshot.native_ids.len(),
        },
    )?;
    if kept_snapshot_token_count == 0 {
        return Err(DuAwareCandidateError::FullWindowForbidden { drop_tokens });
    }
    let repair_start_byte = snapshot.native_spans[kept_snapshot_token_count - 1].byte_end;
    timings.span_lookup_wall_ns = span_started.elapsed().as_nanos();

    if repair_start_byte == 0 {
        return Err(DuAwareCandidateError::FullWindowForbidden { drop_tokens });
    }
    if repair_start_byte > snapshot.prefix_len {
        return Err(DuAwareCandidateError::RepairStartOutsidePrefix {
            repair_start_byte,
            prefix_len: snapshot.prefix_len,
        });
    }

    let du_started = Instant::now();
    let snapshot_tail = snapshot
        .du_state
        .reconstruct_validated_range(repair_start_byte, snapshot.prefix_len)
        .map_err(|error| DuAwareCandidateError::SnapshotTailReconstruction(error.to_string()))?;
    timings.du_reconstruct_wall_ns = du_started.elapsed().as_nanos();

    let mut repair_window = Vec::with_capacity(snapshot_tail.len() + delta_bytes.len());
    repair_window.extend_from_slice(&snapshot_tail);
    repair_window.extend_from_slice(delta_bytes);

    let tokenize_started = Instant::now();
    let repair_ids =
        tokenize_repair_window(&repair_window).map_err(DuAwareCandidateError::Tokenize)?;
    timings.repair_tokenize_wall_ns = tokenize_started.elapsed().as_nanos();

    let splice_started = Instant::now();
    let kept_ids = &snapshot.native_ids[..kept_snapshot_token_count];
    let mut ids = Vec::with_capacity(kept_ids.len() + repair_ids.len());
    ids.extend_from_slice(kept_ids);
    ids.extend_from_slice(&repair_ids);
    timings.splice_wall_ns = splice_started.elapsed().as_nanos();
    timings.candidate_total_prep_wall_ns = total_started.elapsed().as_nanos();

    Ok((
        DuAwareRepairCandidate {
            ids,
            drop_tokens,
            kept_snapshot_token_count,
            repair_start_byte,
            snapshot_tail_bytes: snapshot_tail.len(),
            repair_window_bytes: repair_window.len(),
            repair_token_count: repair_ids.len(),
        },
        timings,
    ))
}

/// Construct one V2 candidate from persisted spans and DU structural state.
/// The only native callback is repair-window tokenization; full-prefix
/// detokenization and full-window tokenization are not expressible here.
pub fn construct_du_aware_repair_candidate<Tokenize>(
    snapshot: &DirectIdSnapshotV2,
    delta: &crate::LoadedStream,
    drop_tokens: usize,
    tokenize_repair_window: &mut Tokenize,
) -> Result<(DuAwareRepairCandidate, DuAwareRepairTimings), DuAwareCandidateError>
where
    Tokenize: FnMut(&[u8]) -> Result<Vec<u32>, String>,
{
    snapshot
        .validate()
        .map_err(DuAwareCandidateError::InvalidSnapshot)?;

    let total_started = Instant::now();
    let du_started = Instant::now();
    let delta_bytes = delta
        .reconstruct()
        .map_err(|error| DuAwareCandidateError::InvalidDelta(error.to_string()))?;
    let delta_wall_ns = du_started.elapsed().as_nanos();
    let (candidate, mut timings) = construct_du_aware_candidate_from_delta(
        snapshot,
        &delta_bytes,
        drop_tokens,
        tokenize_repair_window,
    )?;
    timings.du_reconstruct_wall_ns = timings.du_reconstruct_wall_ns.saturating_add(delta_wall_ns);
    timings.candidate_total_prep_wall_ns = total_started.elapsed().as_nanos();
    Ok((candidate, timings))
}

/// Search an inclusive repair bound. The oracle is consulted only after each
/// independently constructed candidate exists and is never passed to the
/// repair tokenizer callback.
pub fn search_exact_du_aware_repair<Tokenize>(
    snapshot: &DirectIdSnapshotV2,
    delta: &crate::LoadedStream,
    oracle_ids: &[u32],
    max_drop_tokens: usize,
    tokenize_repair_window: &mut Tokenize,
) -> Result<DuAwareExactRepair, DuAwareRepairSearchError>
where
    Tokenize: FnMut(&[u8]) -> Result<Vec<u32>, String>,
{
    snapshot
        .validate()
        .map_err(DuAwareCandidateError::InvalidSnapshot)
        .map_err(DuAwareRepairSearchError::Candidate)?;

    let delta_total_started = Instant::now();
    let du_started = Instant::now();
    let reconstructed_delta = delta
        .reconstruct()
        .map_err(|error| DuAwareCandidateError::InvalidDelta(error.to_string()))
        .map_err(DuAwareRepairSearchError::Candidate)?;
    let mut timings = DuAwareRepairTimings {
        du_reconstruct_wall_ns: du_started.elapsed().as_nanos(),
        candidate_total_prep_wall_ns: delta_total_started.elapsed().as_nanos(),
        ..DuAwareRepairTimings::default()
    };
    let mut trials = Vec::new();

    for drop_tokens in 0..=max_drop_tokens {
        match construct_du_aware_candidate_from_delta(
            snapshot,
            &reconstructed_delta,
            drop_tokens,
            tokenize_repair_window,
        ) {
            Ok((candidate, candidate_timings)) => {
                timings.add_assign(&candidate_timings);
                let ids_exact = candidate.ids == oracle_ids;
                trials.push(DuAwareRepairTrial::from_candidate(&candidate, ids_exact));
                if ids_exact {
                    return Ok(DuAwareExactRepair {
                        candidate,
                        reconstructed_delta,
                        timings,
                        trials,
                    });
                }
            }
            Err(error) if error.is_callback_error() => {
                return Err(DuAwareRepairSearchError::Candidate(error));
            }
            Err(error) => trials.push(DuAwareRepairTrial::rejected(drop_tokens, &error)),
        }
    }

    Err(DuAwareRepairSearchError::Exhausted {
        max_drop_tokens,
        timings,
        trials,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn byte_detokenizer(ids: &[u32]) -> Result<Vec<u8>, String> {
        ids.iter()
            .map(|id| u8::try_from(*id).map_err(|error| error.to_string()))
            .collect()
    }

    #[test]
    fn drop_zero_keeps_all_native_snapshot_ids() {
        let mut tokenize = |bytes: &[u8]| Ok(bytes.iter().map(|byte| u32::from(*byte)).collect());
        let mut detokenize = byte_detokenizer;
        let exact = search_exact_repair(
            b"abc",
            b"d",
            &[97, 98, 99],
            &[97, 98, 99, 100],
            3,
            &mut tokenize,
            &mut detokenize,
        )
        .unwrap();

        assert_eq!(exact.candidate.drop_tokens, 0);
        assert_eq!(exact.candidate.ids, [97, 98, 99, 100]);
        assert_eq!(exact.candidate.snapshot_tail_bytes_repaired, 0);
    }

    #[test]
    fn drop_one_repairs_one_snapshot_token() {
        let mut tokenize = |bytes: &[u8]| match bytes {
            b"x" => Ok(vec![120]),
            b"bx" => Ok(vec![900]),
            other => Err(format!("unexpected repair window: {other:?}")),
        };
        let mut detokenize = byte_detokenizer;
        let exact = search_exact_repair(
            b"ab",
            b"x",
            &[97, 98],
            &[97, 900],
            1,
            &mut tokenize,
            &mut detokenize,
        )
        .unwrap();

        assert_eq!(exact.candidate.drop_tokens, 1);
        assert_eq!(exact.candidate.ids, [97, 900]);
        assert_eq!(exact.candidate.snapshot_tail_bytes_repaired, 1);
    }

    #[test]
    fn drop_multiple_repairs_multiple_snapshot_tokens() {
        let mut tokenize = |bytes: &[u8]| match bytes {
            b"x" => Ok(vec![120]),
            b"cx" => Ok(vec![901]),
            b"bcx" => Ok(vec![902]),
            other => Err(format!("unexpected repair window: {other:?}")),
        };
        let mut detokenize = byte_detokenizer;
        let exact = search_exact_repair(
            b"abc",
            b"x",
            &[97, 98, 99],
            &[97, 902],
            2,
            &mut tokenize,
            &mut detokenize,
        )
        .unwrap();

        assert_eq!(exact.candidate.drop_tokens, 2);
        assert_eq!(exact.candidate.ids, [97, 902]);
    }

    #[test]
    fn empty_delta_can_reuse_snapshot_unchanged() {
        let mut tokenize = |bytes: &[u8]| {
            assert!(bytes.is_empty());
            Ok(Vec::new())
        };
        let mut detokenize = byte_detokenizer;
        let exact = search_exact_repair(
            b"ab",
            b"",
            &[97, 98],
            &[97, 98],
            0,
            &mut tokenize,
            &mut detokenize,
        )
        .unwrap();

        assert_eq!(exact.candidate.ids, [97, 98]);
        assert_eq!(exact.candidate.repair_window_bytes, 0);
    }

    #[test]
    fn candidate_splice_order_is_kept_then_repair() {
        let mut tokenize = |bytes: &[u8]| {
            assert_eq!(bytes, b"cd!");
            Ok(vec![700, 701])
        };
        let mut detokenize = |ids: &[u32]| {
            assert_eq!(ids, [10, 20]);
            Ok(b"ab".to_vec())
        };
        let candidate = construct_repair_candidate(
            b"abcd",
            b"!",
            &[10, 20, 30, 40],
            2,
            &mut tokenize,
            &mut detokenize,
        )
        .unwrap();

        assert_eq!(candidate.ids, [10, 20, 700, 701]);
    }

    #[test]
    fn invalid_kept_prefix_bytes_are_rejected_before_tokenize() {
        let mut tokenize_called = false;
        let mut tokenize = |_bytes: &[u8]| {
            tokenize_called = true;
            Ok(Vec::new())
        };
        let mut detokenize = |_ids: &[u32]| Ok(b"wrong".to_vec());
        let error = construct_repair_candidate(
            b"prefix",
            b"delta",
            &[1, 2],
            1,
            &mut tokenize,
            &mut detokenize,
        )
        .unwrap_err();

        assert!(matches!(error, CandidateError::KeptBytesNotPrefix { .. }));
        assert!(!tokenize_called);
    }

    #[test]
    fn max_drop_exhaustion_fails_closed() {
        let mut seen = Vec::new();
        let mut tokenize = |bytes: &[u8]| {
            seen.push(bytes.to_vec());
            Ok(vec![999])
        };
        let mut detokenize = byte_detokenizer;
        let error = search_exact_repair(
            b"abc",
            b"x",
            &[97, 98, 99],
            &[97, 902],
            1,
            &mut tokenize,
            &mut detokenize,
        )
        .unwrap_err();

        match error {
            RepairSearchError::Exhausted {
                max_drop_tokens,
                trials,
            } => {
                assert_eq!(max_drop_tokens, 1);
                assert_eq!(trials.len(), 2);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(seen, [b"x".to_vec(), b"cx".to_vec()]);
    }

    #[test]
    fn full_nonempty_prefix_window_is_never_tokenized() {
        let mut tokenize_called = false;
        let mut tokenize = |_bytes: &[u8]| {
            tokenize_called = true;
            Ok(Vec::new())
        };
        let mut detokenize = |_ids: &[u32]| Ok(Vec::new());
        let error = construct_repair_candidate(b"a", b"b", &[1], 1, &mut tokenize, &mut detokenize)
            .unwrap_err();

        assert!(matches!(error, CandidateError::FullWindowForbidden { .. }));
        assert!(!tokenize_called);
    }

    fn loaded_stream(parts: &[&[u8]]) -> crate::LoadedStream {
        let mut bounds = Vec::with_capacity(parts.len() + 1);
        bounds.push(0);
        for part in parts {
            bounds.push(bounds.last().copied().unwrap() + part.len());
        }
        crate::LoadedStream {
            ids: (0..parts.len()).map(|id| id as u64).collect(),
            parts: parts.iter().map(|part| part.to_vec()).collect(),
            bounds,
        }
    }

    fn v2_snapshot() -> DirectIdSnapshotV2 {
        DirectIdSnapshotV2::new(
            b"abcd",
            vec![10, 20],
            vec![
                crate::TokenSpan {
                    id: 10,
                    byte_start: 0,
                    byte_end: 2,
                    bytes_hex: "6162".to_string(),
                },
                crate::TokenSpan {
                    id: 20,
                    byte_start: 2,
                    byte_end: 4,
                    bytes_hex: "6364".to_string(),
                },
            ],
            loaded_stream(&[b"abc", b"d"]),
        )
        .unwrap()
    }

    #[test]
    fn v2_snapshot_is_serializable_and_revalidates() {
        let snapshot = v2_snapshot();
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        let decoded: DirectIdSnapshotV2 = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(decoded, snapshot);
        decoded.validate().unwrap();
    }

    #[test]
    fn v2_repair_uses_native_span_and_du_tail_across_lexeme_boundary() {
        let snapshot = v2_snapshot();
        let delta = loaded_stream(&[b"!"]);
        let mut windows = Vec::new();
        let mut tokenize = |bytes: &[u8]| {
            windows.push(bytes.to_vec());
            match bytes {
                b"!" => Ok(vec![99]),
                b"cd!" => Ok(vec![700, 701]),
                other => Err(format!("unexpected repair window: {other:?}")),
            }
        };

        let repair =
            search_exact_du_aware_repair(&snapshot, &delta, &[10, 700, 701], 1, &mut tokenize)
                .unwrap();

        assert_eq!(windows, [b"!".to_vec(), b"cd!".to_vec()]);
        assert_eq!(repair.reconstructed_delta.as_slice(), b"!");
        assert_eq!(repair.candidate.drop_tokens, 1);
        assert_eq!(repair.candidate.repair_start_byte, 2);
        assert_eq!(repair.candidate.snapshot_tail_bytes, 2);
        assert_eq!(repair.candidate.ids, [10, 700, 701]);
    }

    #[test]
    fn v2_candidate_splices_exact_kept_native_ids_before_repair_ids() {
        let snapshot = v2_snapshot();
        let delta = loaded_stream(&[b"!"]);
        let mut tokenize = |bytes: &[u8]| {
            assert_eq!(bytes, b"cd!");
            Ok(vec![800, 801])
        };

        let (candidate, _) =
            construct_du_aware_repair_candidate(&snapshot, &delta, 1, &mut tokenize).unwrap();

        assert_eq!(candidate.ids, [10, 800, 801]);
        assert_eq!(candidate.kept_snapshot_token_count, 1);
    }

    #[test]
    fn v2_empty_du_delta_reuses_snapshot_ids() {
        let snapshot = v2_snapshot();
        let delta = loaded_stream(&[]);
        let mut tokenize = |bytes: &[u8]| {
            assert!(bytes.is_empty());
            Ok(Vec::new())
        };

        let repair =
            search_exact_du_aware_repair(&snapshot, &delta, &snapshot.native_ids, 0, &mut tokenize)
                .unwrap();

        assert!(repair.reconstructed_delta.is_empty());
        assert_eq!(repair.candidate.repair_start_byte, snapshot.prefix_len);
        assert_eq!(repair.candidate.ids, snapshot.native_ids);
    }

    #[test]
    fn v2_snapshot_rejects_native_span_id_or_bytes_mismatch() {
        let mut wrong_id = v2_snapshot();
        wrong_id.native_spans[1].id = 21;
        assert!(wrong_id.validate().is_err());

        let mut wrong_bytes = v2_snapshot();
        wrong_bytes.native_spans[1].bytes_hex = "7879".to_string();
        assert!(wrong_bytes.validate().is_err());
    }

    #[test]
    fn v2_full_prefix_window_is_rejected_without_tokenizing() {
        let snapshot = v2_snapshot();
        let delta = loaded_stream(&[b"!"]);
        let mut tokenize_called = false;
        let mut tokenize = |_bytes: &[u8]| {
            tokenize_called = true;
            Ok(Vec::new())
        };

        let error =
            construct_du_aware_repair_candidate(&snapshot, &delta, 2, &mut tokenize).unwrap_err();

        assert!(matches!(
            error,
            DuAwareCandidateError::FullWindowForbidden { drop_tokens: 2 }
        ));
        assert!(!tokenize_called);
    }

    #[test]
    fn v2_invalid_du_delta_fails_before_tokenizing() {
        let snapshot = v2_snapshot();
        let delta = crate::LoadedStream {
            ids: vec![0],
            parts: vec![b"!".to_vec()],
            bounds: vec![0, 2],
        };
        let mut tokenize_called = false;
        let mut tokenize = |_bytes: &[u8]| {
            tokenize_called = true;
            Ok(Vec::new())
        };

        let error =
            construct_du_aware_repair_candidate(&snapshot, &delta, 0, &mut tokenize).unwrap_err();

        assert!(matches!(error, DuAwareCandidateError::InvalidDelta(_)));
        assert!(!tokenize_called);
    }

    #[test]
    fn v2_search_exhaustion_fails_closed() {
        let snapshot = v2_snapshot();
        let delta = loaded_stream(&[b"!"]);
        let mut tokenize = |_bytes: &[u8]| Ok(vec![999]);

        let error = search_exact_du_aware_repair(&snapshot, &delta, &[10, 700], 1, &mut tokenize)
            .unwrap_err();

        match error {
            DuAwareRepairSearchError::Exhausted {
                max_drop_tokens,
                trials,
                ..
            } => {
                assert_eq!(max_drop_tokens, 1);
                assert_eq!(trials.len(), 2);
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
