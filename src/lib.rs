use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

mod bridge;
mod du_remap;

pub use bridge::*;
pub use du_remap::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanRecord {
    pub source_id: Option<u64>,
    pub source_hex: String,
    pub du_ids: Vec<u64>,
    pub exact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenSpan {
    pub id: u64,
    pub byte_start: usize,
    pub byte_end: usize,
    pub bytes_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanSample {
    pub target_pos: usize,
    pub target_id_dense: u64,
    pub target_byte_start: usize,
    pub target_byte_end: usize,
    pub target_bytes_hex: String,
    pub du_start_pos: usize,
    pub du_end_pos_exclusive: usize,
    pub du_span_len: usize,
    pub target_starts_on_du_boundary: bool,
    pub target_ends_on_du_boundary: bool,
    pub du_context: Vec<TokenSpan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanLedgerReport {
    pub mode: String,
    pub target: String,
    pub claim: String,
    pub not_claimed: String,
    pub timestamp: String,
    pub nereids_version: String,
    pub input_size: usize,
    pub input_sha256: String,
    pub wall_time_ms: u128,
    pub bytes_tested: usize,
    pub target_vocab_size_used: usize,
    pub target_tokens_tested: usize,
    pub du_tokens_covering_prefix: usize,
    pub target_decode_equals_raw_prefix: bool,
    pub du_decode_equals_raw_prefix: bool,
    pub target_decode_first_mismatch: Option<usize>,
    pub du_decode_first_mismatch: Option<usize>,
    pub target_tokens_exactly_on_du_boundaries: usize,
    pub target_tokens_cross_or_cut_du_tokens: usize,
    pub target_tokens_start_inside_du_token: usize,
    pub target_tokens_end_inside_du_token: usize,
    pub max_du_tokens_covered_by_one_target_token: usize,
    pub fraction_exact_boundary: Option<f64>,
    pub exact_reconstruction: bool,
    pub span_samples: Vec<SpanSample>,
}

#[derive(Debug, Clone)]
pub struct LoadedStream {
    pub ids: Vec<u64>,
    pub parts: Vec<Vec<u8>>,
    pub bounds: Vec<usize>,
}

pub fn bytes_to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

pub fn hex_to_bytes(hex_s: &str) -> Result<Vec<u8>> {
    Ok(hex::decode(hex_s)?)
}

pub fn assert_exact_rebuild(original: &[u8], rebuilt: &[u8]) -> Result<()> {
    if original != rebuilt {
        bail!(
            "exact rebuild failed: byte mismatch at {:?}",
            first_mismatch(original, rebuilt)
        );
    }
    Ok(())
}

pub fn first_mismatch(a: &[u8], b: &[u8]) -> Option<usize> {
    let n = a.len().min(b.len());
    for i in 0..n {
        if a[i] != b[i] {
            return Some(i);
        }
    }
    (a.len() != b.len()).then_some(n)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn decode_base64_unpadded_or_padded(encoded: &str) -> Result<Vec<u8>> {
    let bytes = encoded.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 == 1 {
        bail!("invalid base64 length");
    }

    let mut clean = bytes.to_vec();
    while clean.len() % 4 != 0 {
        clean.push(b'=');
    }

    let padding = clean.iter().rev().take_while(|b| **b == b'=').count();
    if padding > 2 || clean[..clean.len() - padding].contains(&b'=') {
        bail!("invalid base64 padding");
    }

    let mut out = Vec::with_capacity(clean.len() / 4 * 3 - padding);
    for chunk in clean.chunks_exact(4) {
        let a = base64_value(chunk[0]).ok_or_else(|| anyhow!("invalid base64 character"))?;
        let b = base64_value(chunk[1]).ok_or_else(|| anyhow!("invalid base64 character"))?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            base64_value(chunk[2]).ok_or_else(|| anyhow!("invalid base64 character"))?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            base64_value(chunk[3]).ok_or_else(|| anyhow!("invalid base64 character"))?
        };

        if chunk[2] == b'=' && chunk[3] != b'=' {
            bail!("invalid base64 padding");
        }
        if chunk[2] == b'=' && (b & 0x0f) != 0 {
            bail!("non-zero base64 padding bits");
        }
        if chunk[3] == b'=' && chunk[2] != b'=' && (c & 0x03) != 0 {
            bail!("non-zero base64 padding bits");
        }

        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            out.push((c << 6) | d);
        }
    }
    Ok(out)
}

/// Decode the control-sequence representation used by merged.dict.
///
/// The legacy format uses control bytes for newlines, tabs, repeated spaces,
/// and UTF-8 text stored as base64. Invalid control sequences are rejected so
/// malformed dictionary entries cannot silently become different bytes.
pub fn decode_du_lexeme(input: &str) -> Result<Vec<u8>> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::new();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '\u{0004}' => {
                let start = i + 1;
                let end = chars[start..]
                    .iter()
                    .position(|c| *c == '\u{0005}')
                    .map(|offset| start + offset)
                    .ok_or_else(|| anyhow!("unterminated base64 lexeme marker"))?;
                let encoded: String = chars[start..end].iter().collect();
                let decoded = decode_base64_unpadded_or_padded(&encoded)?;
                let decoded = String::from_utf8(decoded)
                    .context("base64 lexeme payload is not valid UTF-8")?;
                out.push_str(&decoded);
                i = end + 1;
            }
            '\u{0003}' => {
                out.push('\n');
                i += 1;
            }
            '\u{0001}' => {
                out.push('\t');
                i += 1;
            }
            '\u{0002}' => {
                let start = i + 1;
                let end = chars[start..]
                    .iter()
                    .position(|c| *c == '\u{0002}')
                    .map(|offset| start + offset)
                    .ok_or_else(|| anyhow!("unterminated repeated-space marker"))?;
                let count: String = chars[start..end].iter().collect();
                if count.is_empty() || !count.chars().all(|c| c.is_ascii_digit()) {
                    bail!("invalid repeated-space marker");
                }
                let count: usize = count.parse().context("repeated-space count overflow")?;
                out.extend(std::iter::repeat(' ').take(count));
                i = end + 1;
            }
            '\u{0000}' => bail!("NUL is not valid in a DU lexeme"),
            c => {
                out.push(c);
                i += 1;
            }
        }
    }

    Ok(out.into_bytes())
}

pub fn load_du_dict(path: impl AsRef<Path>) -> Result<Vec<Vec<u8>>> {
    let path = path.as_ref();
    let text = fs::read_to_string(path)
        .with_context(|| format!("read DU dictionary {}", path.display()))?;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let lines = text.split('\n').collect::<Vec<_>>();
    let mut entries = Vec::new();
    for (line_no, line) in lines.iter().copied().enumerate() {
        if line_no + 1 == lines.len() && line.is_empty() && text.ends_with('\n') {
            continue;
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        entries.push(decode_du_lexeme(line).with_context(|| {
            format!(
                "decode DU dictionary entry {} in {}",
                line_no + 1,
                path.display()
            )
        })?);
    }
    Ok(entries)
}

pub fn load_model_bin(path: impl AsRef<Path>) -> Result<Vec<Vec<u8>>> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| format!("read model.bin {}", path.display()))?;
    if bytes.len() < 4 {
        bail!("model.bin is missing its vocabulary count");
    }
    let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let mut offset = 4;
    let mut vocab = Vec::with_capacity(count);
    for _ in 0..count {
        if bytes.len() - offset < 4 {
            bail!("model.bin ends before token length");
        }
        let len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| anyhow!("model.bin token length overflow"))?;
        if end > bytes.len() {
            bail!("model.bin ends inside token payload");
        }
        vocab.push(bytes[offset..end].to_vec());
        offset = end;
    }
    if offset != bytes.len() {
        bail!("model.bin trailing bytes: {}", bytes.len() - offset);
    }
    Ok(vocab)
}

fn read_fixed_u16(path: &Path) -> Result<Vec<u16>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() % 2 != 0 {
        bail!("bad u16 size: {}", path.display());
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}

fn read_fixed_u32(path: &Path) -> Result<Vec<u32>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() % 4 != 0 {
        bail!("bad u32 size: {}", path.display());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn load_gmap(path: &Path, width: usize) -> Result<Vec<u32>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let step = if width == 24 { 3 } else { 4 };
    if bytes.len() % step != 0 {
        bail!("bad gmap{} size: {}", width, path.display());
    }
    let mut out = Vec::with_capacity(bytes.len() / step);
    for chunk in bytes.chunks_exact(step) {
        out.push(if width == 24 {
            u32::from(chunk[0]) | (u32::from(chunk[1]) << 8) | (u32::from(chunk[2]) << 16)
        } else {
            u32::from_le_bytes(chunk.try_into().unwrap())
        });
    }
    Ok(out)
}

fn sorted_files(path: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(path)
        .with_context(|| format!("read directory {}", path.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|x| x.to_str()) == Some(extension))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

pub fn load_du_stream(du_run: impl AsRef<Path>, max_bytes: usize) -> Result<(LoadedStream, usize)> {
    let du_run = du_run.as_ref();
    let vocab = load_du_dict(du_run.join("merged.dict"))?;
    let local_dir = du_run.join("local_u16");
    let (gmap_dir, width, suffix) = if du_run.join("gmap24").is_dir() {
        (du_run.join("gmap24"), 24, "gmap24")
    } else if du_run.join("gmap32").is_dir() {
        (du_run.join("gmap32"), 32, "gmap32")
    } else {
        bail!("missing gmap24/gmap32 in {}", du_run.display());
    };

    let mut ids = Vec::new();
    let mut parts = Vec::new();
    let mut byte_len = 0;
    for u16_path in sorted_files(&local_dir, "u16")? {
        let stem = u16_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let gmap_path = gmap_dir.join(format!("{stem}.{suffix}"));
        let gmap = load_gmap(&gmap_path, width)?;
        for local_id in read_fixed_u16(&u16_path)? {
            let global_id = *gmap
                .get(usize::from(local_id))
                .ok_or_else(|| anyhow!("local ID {} outside {}", local_id, gmap_path.display()))?;
            let token = vocab
                .get(global_id as usize)
                .ok_or_else(|| anyhow!("global ID {} outside DU vocabulary", global_id))?
                .clone();
            byte_len += token.len();
            ids.push(u64::from(global_id));
            parts.push(token);
            if byte_len >= max_bytes {
                return Ok((make_loaded_stream(ids, parts), vocab.len()));
            }
        }
    }
    Ok((make_loaded_stream(ids, parts), vocab.len()))
}

pub fn load_target_stream(
    tik_remap: impl AsRef<Path>,
    max_bytes: usize,
) -> Result<(LoadedStream, usize)> {
    let tik_remap = tik_remap.as_ref();
    let vocab = load_model_bin(tik_remap.join("model.bin"))?;
    let mut ids = Vec::new();
    let mut parts = Vec::new();
    let mut byte_len = 0;
    for target_id in read_fixed_u32(&tik_remap.join("tokens.u32"))? {
        let token = vocab
            .get(target_id as usize)
            .ok_or_else(|| anyhow!("target ID {} outside target vocabulary", target_id))?
            .clone();
        byte_len += token.len();
        ids.push(u64::from(target_id));
        parts.push(token);
        if byte_len >= max_bytes {
            break;
        }
    }
    Ok((make_loaded_stream(ids, parts), vocab.len()))
}

fn make_loaded_stream(ids: Vec<u64>, parts: Vec<Vec<u8>>) -> LoadedStream {
    let mut bounds = Vec::with_capacity(parts.len() + 1);
    bounds.push(0);
    for part in &parts {
        bounds.push(bounds.last().copied().unwrap_or(0) + part.len());
    }
    LoadedStream { ids, parts, bounds }
}

fn upper_bound(values: &[usize], needle: usize) -> usize {
    let mut low = 0;
    let mut high = values.len();
    while low < high {
        let mid = (low + high) / 2;
        if values[mid] <= needle {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

fn lower_bound(values: &[usize], needle: usize) -> usize {
    let mut low = 0;
    let mut high = values.len();
    while low < high {
        let mid = (low + high) / 2;
        if values[mid] < needle {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

/// Return the DU token range covered by a target byte span.
///
/// The returned tuple is `(start, end_exclusive, count, starts_on_boundary,
/// ends_on_boundary)`. The boundary checks use the complete DU boundary list;
/// callers that operate on a truncated prefix should pass the prefix's
/// boundary list.
pub fn target_du_span(
    du_bounds: &[usize],
    target_start: usize,
    target_end: usize,
) -> (usize, usize, usize, bool, bool) {
    let start = upper_bound(du_bounds, target_start).saturating_sub(1);
    let end = lower_bound(du_bounds, target_end);
    let starts_on_boundary = du_bounds.binary_search(&target_start).is_ok();
    let ends_on_boundary = du_bounds.binary_search(&target_end).is_ok();
    (
        start,
        end,
        end.saturating_sub(start),
        starts_on_boundary,
        ends_on_boundary,
    )
}

fn token_spans(stream: &LoadedStream) -> Vec<TokenSpan> {
    stream
        .ids
        .iter()
        .enumerate()
        .map(|(index, id)| TokenSpan {
            id: *id,
            byte_start: stream.bounds[index],
            byte_end: stream.bounds[index + 1],
            bytes_hex: bytes_to_hex(&stream.parts[index]),
        })
        .collect()
}

pub fn span_ledger_report(
    target: &LoadedStream,
    du: &LoadedStream,
    raw: &[u8],
    target_vocab_size: usize,
    max_bytes: usize,
    wall_time_ms: u128,
) -> SpanLedgerReport {
    let target_total = *target.bounds.last().unwrap_or(&0);
    let du_total = *du.bounds.last().unwrap_or(&0);
    let limit = raw.len().min(target_total).min(du_total).min(max_bytes);
    let raw_prefix = &raw[..limit];
    let target_bytes = target
        .parts
        .iter()
        .flatten()
        .copied()
        .take(limit)
        .collect::<Vec<_>>();
    let du_bytes = du
        .parts
        .iter()
        .flatten()
        .copied()
        .take(limit)
        .collect::<Vec<_>>();

    let target_count = upper_bound(&target.bounds, limit).saturating_sub(1);
    let du_count = upper_bound(&du.bounds, limit).saturating_sub(1);
    let du_boundary_set = &du.bounds[..=du_count.min(du.bounds.len().saturating_sub(1))];
    let mut exact = 0;
    let mut cut_or_cross = 0;
    let mut start_inside = 0;
    let mut end_inside = 0;
    let mut max_du_span = 0;
    let mut samples = Vec::new();
    let du_spans = token_spans(du);

    for ti in 0..target_count {
        let a = target.bounds[ti];
        let b = target.bounds[ti + 1];
        let (ds, de, span, starts_on_boundary, ends_on_boundary) = target_du_span(&du.bounds, a, b);
        max_du_span = max_du_span.max(span);
        let starts_on_boundary = starts_on_boundary && du_boundary_set.binary_search(&a).is_ok();
        let ends_on_boundary = ends_on_boundary && du_boundary_set.binary_search(&b).is_ok();

        if starts_on_boundary && ends_on_boundary {
            exact += 1;
        } else {
            cut_or_cross += 1;
            if !starts_on_boundary {
                start_inside += 1;
            }
            if !ends_on_boundary {
                end_inside += 1;
            }
            if samples.len() < 80 {
                let context_start = ds.saturating_sub(3);
                let context_end = (de + 3).min(du.ids.len());
                samples.push(SpanSample {
                    target_pos: ti,
                    target_id_dense: target.ids[ti],
                    target_byte_start: a,
                    target_byte_end: b,
                    target_bytes_hex: bytes_to_hex(&target.parts[ti]),
                    du_start_pos: ds,
                    du_end_pos_exclusive: de,
                    du_span_len: span,
                    target_starts_on_du_boundary: starts_on_boundary,
                    target_ends_on_du_boundary: ends_on_boundary,
                    du_context: du_spans[context_start..context_end].to_vec(),
                });
            }
        }
    }

    let target_mismatch = first_mismatch(&target_bytes, raw_prefix);
    let du_mismatch = first_mismatch(&du_bytes, raw_prefix);
    SpanLedgerReport {
        mode: "Nereid v0 fixed span ledger".to_string(),
        target: "bench-pre-v2 dense cl100k_base used-vocab remap".to_string(),
        claim: "align direct target-remap stream and decoded DU stream by raw byte spans"
            .to_string(),
        not_claimed: "official model IDs or full target tokenizer generation".to_string(),
        timestamp: timestamp(),
        nereids_version: version().to_string(),
        input_size: raw.len(),
        input_sha256: sha256_hex(raw),
        wall_time_ms,
        bytes_tested: limit,
        target_vocab_size_used: target_vocab_size,
        target_tokens_tested: target_count,
        du_tokens_covering_prefix: du_count,
        target_decode_equals_raw_prefix: target_mismatch.is_none(),
        du_decode_equals_raw_prefix: du_mismatch.is_none(),
        target_decode_first_mismatch: target_mismatch,
        du_decode_first_mismatch: du_mismatch,
        target_tokens_exactly_on_du_boundaries: exact,
        target_tokens_cross_or_cut_du_tokens: cut_or_cross,
        target_tokens_start_inside_du_token: start_inside,
        target_tokens_end_inside_du_token: end_inside,
        max_du_tokens_covered_by_one_target_token: max_du_span,
        fraction_exact_boundary: (target_count > 0).then_some(exact as f64 / target_count as f64),
        exact_reconstruction: target_mismatch.is_none() && du_mismatch.is_none(),
        span_samples: samples,
    }
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(ids: &[u64], parts: &[&[u8]]) -> LoadedStream {
        make_loaded_stream(
            ids.to_vec(),
            parts.iter().map(|part| part.to_vec()).collect(),
        )
    }

    #[test]
    fn decodes_du_control_sequences() {
        assert_eq!(
            decode_du_lexeme("a\u{0003}b\u{0001}c\u{0002}3\u{0002}").unwrap(),
            b"a\nb\tc   "
        );
        assert_eq!(
            decode_du_lexeme("\u{0004}5LiW55WM\u{0005}").unwrap(),
            "世界".as_bytes()
        );
    }

    #[test]
    fn rejects_malformed_lexemes() {
        assert!(decode_du_lexeme("\u{0004}not base64\u{0005}").is_err());
        assert!(decode_du_lexeme("\u{0002}oops\u{0002}").is_err());
        assert!(decode_du_lexeme("\u{0004}abc").is_err());
    }

    #[test]
    fn fixture_rebuild_is_exact() {
        let fixture = include_bytes!("../fixtures/multilingual.txt");
        assert_exact_rebuild(fixture, fixture).unwrap();
        assert_eq!(sha256_hex(fixture).len(), 64);
    }

    #[test]
    fn span_ledger_detects_alignment_and_cuts() {
        let du = stream(&[10, 11, 12], &[b"ab", b"cde", b"fg"]);
        let target = stream(&[20, 21, 22], &[b"ab", b"cd", b"efg"]);
        let report = span_ledger_report(&target, &du, b"abcdefg", 3, 100, 0);
        assert_eq!(report.target_tokens_exactly_on_du_boundaries, 1);
        assert_eq!(report.target_tokens_start_inside_du_token, 1);
        assert_eq!(report.target_tokens_end_inside_du_token, 1);
        assert_eq!(report.max_du_tokens_covered_by_one_target_token, 2);
        assert!(report.exact_reconstruction);
    }

    #[test]
    fn span_ledger_handles_target_crossing_multiple_du_tokens() {
        let du = stream(&[1, 2, 3], &[b"a", b"b", b"c"]);
        let target = stream(&[9], &[b"abc"]);
        let report = span_ledger_report(&target, &du, b"abc", 1, 100, 0);
        assert_eq!(report.target_tokens_exactly_on_du_boundaries, 1);
        assert_eq!(report.max_du_tokens_covered_by_one_target_token, 3);
    }
}
