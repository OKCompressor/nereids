//! Precomputed DU-global-ID to target-vocabulary-ID remapping.
//!
//! This is deliberately a structural byte bridge. It does not execute a
//! target tokenizer and it does not claim that greedy IDs are the target
//! tokenizer's canonical segmentation.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
struct GreedyTrieNode {
    terminal_id: Option<u32>,
    children: Vec<(u8, usize)>,
}

#[derive(Debug, Clone)]
struct GreedyTrie {
    roots: [Option<usize>; 256],
    nodes: Vec<GreedyTrieNode>,
}

impl GreedyTrie {
    fn new() -> Self {
        Self {
            roots: [None; 256],
            nodes: Vec::new(),
        }
    }

    fn alloc_node(&mut self) -> usize {
        let index = self.nodes.len();
        self.nodes.push(GreedyTrieNode::default());
        index
    }

    fn insert(&mut self, token: &[u8], id: u32) {
        debug_assert!(!token.is_empty());

        let first = token[0] as usize;

        let mut node_index = match self.roots[first] {
            Some(index) => index,
            None => {
                let index = self.alloc_node();
                self.roots[first] = Some(index);
                index
            }
        };

        for &byte in &token[1..] {
            let next = self.nodes[node_index]
                .children
                .iter()
                .find_map(|(edge, index)| (*edge == byte).then_some(*index));

            node_index = match next {
                Some(index) => index,
                None => {
                    let index = self.alloc_node();
                    self.nodes[node_index].children.push((byte, index));
                    index
                }
            };
        }

        // Preserve the old greedy tie-break:
        // longest token first, then lowest native ID for duplicate bytes.
        let terminal = &mut self.nodes[node_index].terminal_id;
        if terminal.as_ref().map_or(true, |old| id < *old) {
            *terminal = Some(id);
        }
    }

    fn finish(&mut self) {
        for node in &mut self.nodes {
            node.children.sort_unstable_by_key(|(byte, _)| *byte);
        }
    }

    fn longest_match(&self, bytes: &[u8], offset: usize) -> Option<(u32, usize)> {
        let first = *bytes.get(offset)?;
        let mut node_index = self.roots[first as usize]?;
        let mut length = 1usize;

        let mut best = self.nodes[node_index].terminal_id.map(|id| (id, length));

        while offset + length < bytes.len() {
            let byte = bytes[offset + length];
            let node = &self.nodes[node_index];

            let child_position = match node.children.binary_search_by_key(&byte, |(edge, _)| *edge)
            {
                Ok(position) => position,
                Err(_) => break,
            };

            node_index = node.children[child_position].1;
            length += 1;

            if let Some(id) = self.nodes[node_index].terminal_id {
                best = Some((id, length));
            }
        }

        best
    }
}

/// Target vocabulary bytes indexed by their preserved target IDs.
#[derive(Debug, Clone)]
pub struct TargetVocab {
    pub tokens: Vec<Vec<u8>>,
    pub first_byte_buckets: Vec<Vec<u32>>,
    pub max_token_bytes: usize,
    pub model_bin_bytes: u64,
    greedy_trie: GreedyTrie,
}

impl TargetVocab {
    /// Load the existing Phase 1 `model.bin` format and retain index == ID.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let tokens = crate::load_model_bin(path)?;
        let model_bin_bytes = fs::metadata(path)
            .with_context(|| format!("stat target model.bin {}", path.display()))?
            .len();
        Self::from_tokens_with_model_size(tokens, model_bin_bytes)
    }

    pub fn from_tokens(tokens: Vec<Vec<u8>>) -> Result<Self> {
        Self::from_tokens_with_model_size(tokens, 0)
    }

    fn from_tokens_with_model_size(tokens: Vec<Vec<u8>>, model_bin_bytes: u64) -> Result<Self> {
        let mut first_byte_buckets = vec![Vec::new(); 256];
        let mut max_token_bytes = 0;
        let mut greedy_trie = GreedyTrie::new();

        for (id, token) in tokens.iter().enumerate() {
            if token.is_empty() {
                continue;
            }
            let id = u32::try_from(id).context("target vocabulary ID exceeds u32")?;
            first_byte_buckets[token[0] as usize].push(id);
            max_token_bytes = max_token_bytes.max(token.len());
            greedy_trie.insert(token, id);
        }
        for bucket in &mut first_byte_buckets {
            bucket.sort_by(|left, right| {
                tokens[*right as usize]
                    .len()
                    .cmp(&tokens[*left as usize].len())
                    .then_with(|| left.cmp(right))
            });
        }
        greedy_trie.finish();

        Ok(Self {
            tokens,
            first_byte_buckets,
            max_token_bytes,
            model_bin_bytes,
            greedy_trie,
        })
    }

    pub fn entries(&self) -> usize {
        self.tokens.len()
    }

    pub fn token_bytes(&self, id: u32) -> Result<&[u8]> {
        self.tokens
            .get(id as usize)
            .map(Vec::as_slice)
            .ok_or_else(|| anyhow!("target vocabulary ID {} is out of range", id))
    }

    /// Longest-byte-match greedy segmentation using the target byte trie.
    pub fn greedy_encode(&self, bytes: &[u8]) -> Result<Vec<u32>> {
        let mut ids = Vec::new();
        let mut offset = 0;

        while offset < bytes.len() {
            let (id, token_len) =
                self.greedy_trie
                    .longest_match(bytes, offset)
                    .ok_or_else(|| {
                        anyhow!(
                            "no target vocabulary token at byte offset {} (byte 0x{:02x})",
                            offset,
                            bytes[offset]
                        )
                    })?;

            ids.push(id);
            offset += token_len;
        }

        Ok(ids)
    }

    /// Decode preserved target IDs back into their target-vocabulary bytes.
    pub fn decode(&self, ids: &[u32]) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        for id in ids {
            bytes.extend_from_slice(self.token_bytes(*id)?);
        }
        Ok(bytes)
    }
}

/// Packed CSR-style DU-global-ID to target-ID expansion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecomputedDuTargetMap {
    pub offsets: Vec<u64>,
    pub ids: Vec<u32>,
}

impl PrecomputedDuTargetMap {
    /// Build the map once from exact DU dictionary bytes.
    pub fn build(du_vocab: &[Vec<u8>], target_vocab: &TargetVocab) -> Result<Self> {
        let mut offsets = Vec::with_capacity(du_vocab.len() + 1);
        let mut ids = Vec::new();
        offsets.push(0);
        for (gid, lexeme) in du_vocab.iter().enumerate() {
            let expansion = target_vocab.greedy_encode(lexeme).with_context(|| {
                format!("greedy target segmentation for DU dictionary gid {}", gid)
            })?;
            let decoded = target_vocab.decode(&expansion).with_context(|| {
                format!("decode target expansion for DU dictionary gid {}", gid)
            })?;
            if decoded != *lexeme {
                bail!(
                    "target expansion changed bytes for DU dictionary gid {}",
                    gid
                );
            }
            ids.extend_from_slice(&expansion);
            offsets.push(u64::try_from(ids.len()).context("map ID payload exceeds u64")?);
        }
        let map = Self { offsets, ids };
        map.validate_packed(du_vocab.len())?;
        Ok(map)
    }

    /// Return the packed target-ID expansion for one DU global ID.
    pub fn lookup(&self, gid: u32) -> Result<&[u32]> {
        let gid = gid as usize;
        if gid + 1 >= self.offsets.len() {
            bail!("DU global ID {} is outside precomputed map", gid);
        }
        let start = usize::try_from(self.offsets[gid]).context("map offset exceeds usize")?;
        let end = usize::try_from(self.offsets[gid + 1]).context("map offset exceeds usize")?;
        self.ids
            .get(start..end)
            .ok_or_else(|| anyhow!("invalid packed map slice for DU global ID {}", gid))
    }

    pub fn validate_packed(&self, du_vocab_entries: usize) -> Result<()> {
        if self.offsets.len() != du_vocab_entries + 1 {
            bail!(
                "packed map has {} offsets for {} DU entries",
                self.offsets.len(),
                du_vocab_entries
            );
        }
        if self.offsets.first().copied() != Some(0) {
            bail!("packed map must start at offset zero");
        }
        if self.offsets.windows(2).any(|window| window[0] > window[1]) {
            bail!("packed map offsets are not monotonic");
        }
        if self.offsets.last().copied() != Some(self.ids.len() as u64) {
            bail!("packed map final offset does not equal ID payload length");
        }
        Ok(())
    }

    pub fn map_target_ids(&self) -> u64 {
        self.ids.len() as u64
    }

    pub fn map_payload_bytes(&self) -> u64 {
        self.ids.len() as u64 * std::mem::size_of::<u32>() as u64
    }

    pub fn map_bytes(&self) -> u64 {
        self.offsets.len() as u64 * std::mem::size_of::<u64>() as u64 + self.map_payload_bytes()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuRemapReceipt {
    pub mode: String,
    pub claim: String,
    pub not_claimed: String,
    pub du_run: String,
    pub target_model_bin: String,
    pub raw: Option<String>,
    pub du_vocab_entries: usize,
    pub target_vocab_entries: usize,
    pub target_vocab_max_token_bytes: usize,
    pub map_target_ids: u64,
    pub map_payload_bytes: u64,
    pub map_bytes: u64,
    pub raw_bytes: Option<u64>,
    pub model_bin_bytes: u64,
    pub map_bytes_per_raw_bytes: Option<f64>,
    pub map_bytes_per_model_bin_bytes: Option<f64>,
    pub build_wall_ns: u128,
    pub map_build_wall_ns: u128,
    pub remap_wall_ns: u128,
    pub validation_wall_ns: Option<u128>,
    pub unmappable_du_entries: usize,
    pub exact_dictionary_decode: bool,
    pub du_positions_consumed: u64,
    pub emitted_target_ids: u64,
    pub du_bytes: u64,
    pub target_decoded_bytes: u64,
    pub du_lexeme_sha256: String,
    pub emitted_target_bytes_sha256: String,
    pub raw_sha256: Option<String>,
    pub all_hashes_exact: bool,
    pub exact_hash_match: bool,
    pub chunk_target_bytes: u64,
    pub chunks_processed: u64,
    pub input_bytes: u64,
    pub peak_rss_kib: Option<u64>,
    pub bytes_per_sec: Option<f64>,
    pub map_bytes_per_du_entry: Option<f64>,
    pub id_output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamTranslationSummary {
    pub du_positions_consumed: u64,
    pub emitted_target_ids: u64,
    pub du_bytes: u64,
    pub target_decoded_bytes: u64,
    pub unique_du_gids: usize,
    pub du_lexeme_sha256: String,
    pub emitted_target_bytes_sha256: String,
    pub chunk_target_bytes: u64,
    pub chunks_processed: u64,
}

fn hash_hex(hasher: Sha256) -> String {
    format!("{:x}", hasher.finalize())
}

fn sorted_u16_files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(path)
        .with_context(|| format!("read DU local_u16 directory {}", path.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("u16"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn read_gmap(path: &Path, width: usize) -> Result<Vec<u32>> {
    let bytes = fs::read(path).with_context(|| format!("read {} gmap", path.display()))?;
    match width {
        24 => {
            if bytes.len() % 3 != 0 {
                bail!("bad gmap24 size: {}", path.display());
            }
            Ok(bytes
                .chunks_exact(3)
                .map(|chunk| {
                    u32::from(chunk[0]) | (u32::from(chunk[1]) << 8) | (u32::from(chunk[2]) << 16)
                })
                .collect())
        }
        32 => {
            if bytes.len() % 4 != 0 {
                bail!("bad gmap32 size: {}", path.display());
            }
            Ok(bytes
                .chunks_exact(4)
                .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
                .collect())
        }
        _ => bail!("unsupported gmap width {}", width),
    }
}

fn read_u16_buf<F: FnMut(u16) -> Result<()>>(
    reader: &mut BufReader<File>,
    mut consume: F,
) -> Result<()> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(());
        }
        if available.len() < 2 {
            bail!("odd-sized u16 stream");
        }
        let count = available.len() / 2;
        for chunk in available[..count * 2].chunks_exact(2) {
            consume(u16::from_le_bytes([chunk[0], chunk[1]]))?;
        }
        reader.consume(count * 2);
    }
}

/// Translate the DU stream using only local IDs, gmap files, and the packed
/// map. Source text is never reconstructed or tokenized.
pub fn translate_du_run<W: Write + ?Sized>(
    du_run: impl AsRef<Path>,
    du_vocab: &[Vec<u8>],
    map: &PrecomputedDuTargetMap,
    target_vocab: &TargetVocab,
    chunk_target_bytes: u64,
    mut id_output: Option<&mut W>,
) -> Result<StreamTranslationSummary> {
    if chunk_target_bytes == 0 {
        bail!("chunk_target_bytes must be greater than zero");
    }
    let du_run = du_run.as_ref();
    let local_dir = du_run.join("local_u16");
    let (gmap_dir, width, suffix) = if du_run.join("gmap24").is_dir() {
        (du_run.join("gmap24"), 24, "gmap24")
    } else if du_run.join("gmap32").is_dir() {
        (du_run.join("gmap32"), 32, "gmap32")
    } else {
        bail!("missing gmap24/gmap32 in {}", du_run.display());
    };

    let mut du_hasher = Sha256::new();
    let mut target_hasher = Sha256::new();
    let mut du_positions_consumed = 0;
    let mut emitted_target_ids = 0;
    let mut du_bytes = 0;
    let mut target_decoded_bytes = 0;
    let mut unique_du_gids = HashSet::new();
    let mut current_chunk_bytes = 0;
    let mut chunks_processed = 0;

    for u16_path in sorted_u16_files(&local_dir)? {
        let stem = u16_path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow!("invalid local_u16 filename {}", u16_path.display()))?;
        let gmap = read_gmap(&gmap_dir.join(format!("{stem}.{suffix}")), width)?;
        let file = File::open(&u16_path)
            .with_context(|| format!("open local_u16 stream {}", u16_path.display()))?;
        let mut reader = BufReader::new(file);
        read_u16_buf(&mut reader, |local_id| {
            let gid = *gmap.get(local_id as usize).ok_or_else(|| {
                anyhow!("local ID {} outside gmap {}", local_id, u16_path.display())
            })?;
            let lexeme = du_vocab
                .get(gid as usize)
                .ok_or_else(|| anyhow!("DU global ID {} outside dictionary", gid))?;
            unique_du_gids.insert(gid);
            du_hasher.update(lexeme);
            du_bytes += lexeme.len() as u64;
            current_chunk_bytes += lexeme.len() as u64;

            let expansion = map.lookup(gid)?;
            for target_id in expansion {
                let target_bytes = target_vocab.token_bytes(*target_id)?;
                target_hasher.update(target_bytes);
                target_decoded_bytes += target_bytes.len() as u64;
                emitted_target_ids += 1;
                if let Some(writer) = id_output.as_deref_mut() {
                    writer.write_all(&target_id.to_le_bytes())?;
                }
            }
            du_positions_consumed += 1;
            if current_chunk_bytes >= chunk_target_bytes {
                chunks_processed += 1;
                current_chunk_bytes = 0;
            }
            Ok(())
        })?;
    }

    if current_chunk_bytes > 0 {
        chunks_processed += 1;
    }

    Ok(StreamTranslationSummary {
        du_positions_consumed,
        emitted_target_ids,
        du_bytes,
        target_decoded_bytes,
        unique_du_gids: unique_du_gids.len(),
        du_lexeme_sha256: hash_hex(du_hasher),
        emitted_target_bytes_sha256: hash_hex(target_hasher),
        chunk_target_bytes,
        chunks_processed,
    })
}

/// Stream-hash an optional raw file without including the work in remap time.
pub fn hash_file(path: impl AsRef<Path>) -> Result<(u64, String)> {
    let path = path.as_ref();
    let file = File::open(path).with_context(|| format!("open raw file {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        bytes += count as u64;
    }
    Ok((bytes, hash_hex(hasher)))
}

pub fn ratio(numerator: u64, denominator: u64) -> Option<f64> {
    (denominator > 0).then_some(numerator as f64 / denominator as f64)
}

/// Linux reports the process high-water resident set in KiB. Other hosts
/// return `None`; the remap remains fully functional without this metric.
pub fn peak_rss_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmHWM:")?.split_whitespace().next()?;
        value.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(tokens: &[&[u8]]) -> TargetVocab {
        TargetVocab::from_tokens(tokens.iter().map(|token| token.to_vec()).collect()).unwrap()
    }

    #[test]
    fn greedy_encode_prefers_longest_then_lowest_duplicate_id() {
        let target = vocab(&[b"a", b"ab", b"ab", b"b"]);
        assert_eq!(target.greedy_encode(b"abab").unwrap(), &[1, 1]);
    }

    #[test]
    fn synthetic_dictionary_builds_exact_remap() {
        let target = vocab(&[b"a", b"b", b"ab", b"c"]);
        let du = vec![b"ab".to_vec(), b"c".to_vec()];
        let map = PrecomputedDuTargetMap::build(&du, &target).unwrap();
        assert_eq!(map.lookup(0).unwrap(), &[2]);
        assert_eq!(map.lookup(1).unwrap(), &[3]);
        assert_eq!(target.decode(map.lookup(0).unwrap()).unwrap(), b"ab");
        assert_eq!(map.offsets, vec![0, 1, 2]);
    }

    #[test]
    fn repeated_gid_reuses_packed_expansion() {
        let target = vocab(&[b"a", b"b", b"ab"]);
        let du = vec![b"ab".to_vec()];
        let map = PrecomputedDuTargetMap::build(&du, &target).unwrap();
        assert!(std::ptr::eq(
            map.lookup(0).unwrap().as_ptr(),
            map.lookup(0).unwrap().as_ptr()
        ));
        assert_eq!(map.lookup(0).unwrap(), &[2]);
    }

    #[test]
    fn lexeme_can_require_multiple_target_ids() {
        let target = vocab(&[b"a", b"b"]);
        let map = PrecomputedDuTargetMap::build(&[b"ab".to_vec()], &target).unwrap();
        assert_eq!(map.lookup(0).unwrap(), &[0, 1]);
        assert_eq!(target.decode(map.lookup(0).unwrap()).unwrap(), b"ab");
    }

    #[test]
    fn unmappable_lexeme_is_rejected() {
        let target = vocab(&[b"a"]);
        let error = PrecomputedDuTargetMap::build(&[b"b".to_vec()], &target).unwrap_err();
        assert!(error.to_string().contains("DU dictionary gid 0"));
    }

    #[test]
    fn packed_offsets_are_valid() {
        let target = vocab(&[b"a", b"b"]);
        let map =
            PrecomputedDuTargetMap::build(&[b"a".to_vec(), b"ab".to_vec(), b"b".to_vec()], &target)
                .unwrap();
        map.validate_packed(3).unwrap();
        assert_eq!(map.map_payload_bytes(), 16);
        assert_eq!(map.map_bytes(), map.offsets.len() as u64 * 8 + 16);
    }

    #[test]
    fn streaming_repeated_ids_reconstruct_exactly() {
        let root = std::env::temp_dir().join(format!(
            "nereids-du-remap-test-{}-{}",
            std::process::id(),
            crate::timestamp()
        ));
        let local = root.join("local_u16");
        let gmap = root.join("gmap24");
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(&gmap).unwrap();
        fs::write(root.join("merged.dict"), b"a\nab\n").unwrap();
        fs::write(local.join("000.u16"), [0u8, 0, 0, 0]).unwrap();
        fs::write(gmap.join("000.gmap24"), [1u8, 0, 0]).unwrap();

        let target = vocab(&[b"a", b"b"]);
        let du = vec![b"a".to_vec(), b"ab".to_vec()];
        let map = PrecomputedDuTargetMap::build(&du, &target).unwrap();
        let mut ids = Vec::new();
        let summary = translate_du_run(&root, &du, &map, &target, 1, Some(&mut ids)).unwrap();
        assert_eq!(summary.du_positions_consumed, 2);
        assert_eq!(summary.emitted_target_ids, 4);
        assert_eq!(summary.chunks_processed, 2);
        assert_eq!(
            summary.du_lexeme_sha256,
            summary.emitted_target_bytes_sha256
        );
        let emitted_ids: Vec<u32> = ids
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .collect();
        assert_eq!(target.decode(&emitted_ids).unwrap(), b"abab");
        assert_eq!(ids, [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
        fs::remove_dir_all(root).unwrap();
    }
}
