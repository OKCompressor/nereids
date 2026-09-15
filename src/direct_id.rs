//! Bounded append repair for canonical native tokenizer IDs.
//!
//! Candidate construction deliberately has no way to request tokenization of
//! `prefix || delta`. It detokenizes a kept native-ID prefix, tokenizes only
//! the remaining snapshot bytes plus the appended delta, and splices the two
//! ID sequences. Full tokenization belongs to the separate verification
//! oracle used by callers.

use serde::Serialize;
use std::error::Error;
use std::fmt;

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
}
