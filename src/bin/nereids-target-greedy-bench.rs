use anyhow::{bail, Context, Result};
use clap::Parser;
use nereids::{hash_file, peak_rss_kib, sha256_hex, TargetVocab};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Parser)]
#[command(
    name = "nereids-target-greedy-bench",
    about = "Benchmark continuous longest-byte-match target-vocabulary projection"
)]
struct Args {
    #[arg(long)]
    target_model_bin: PathBuf,

    #[arg(long)]
    raw: PathBuf,

    /// Zero means the complete input file.
    #[arg(long, default_value_t = 0)]
    max_bytes: usize,

    #[arg(long, default_value_t = 1)]
    warmups: usize,

    #[arg(long, default_value_t = 5)]
    repeats: usize,

    #[arg(long)]
    receipt: PathBuf,

    /// Optional packed little-endian u32 native-ID output.
    #[arg(long)]
    id_output: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct Receipt {
    schema: &'static str,
    mode: &'static str,
    claim: &'static str,
    not_claimed: &'static str,

    raw: String,
    input_bytes: usize,
    input_sha256: String,

    target_model_bin: String,
    target_model_bin_sha256: String,
    target_vocab_entries: usize,
    target_vocab_max_token_bytes: usize,

    warmups: usize,
    repeats: usize,
    wall_ns: Vec<u128>,
    min_wall_ns: u128,
    median_wall_ns: u128,
    mean_wall_ns: f64,
    max_wall_ns: u128,

    median_bytes_per_sec: f64,
    median_mb_per_sec_decimal: f64,

    emitted_target_ids: usize,
    bytes_per_target_id: f64,
    target_ids_sha256_u32le: String,

    decoded_bytes: usize,
    decoded_sha256: String,
    exact_byte_reconstruction: bool,

    peak_rss_kib: Option<u64>,
    id_output: Option<String>,
}

fn median(values: &[u128]) -> u128 {
    let mut values = values.to_vec();
    values.sort_unstable();

    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2
    }
}

fn ids_sha256(ids: &[u32]) -> String {
    let mut hasher = Sha256::new();
    for id in ids {
        hasher.update(id.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn write_ids(path: &PathBuf, ids: &[u32]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let mut writer =
        BufWriter::new(File::create(path).with_context(|| format!("create {}", path.display()))?);

    for id in ids {
        writer.write_all(&id.to_le_bytes())?;
    }

    writer.flush()?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.repeats == 0 {
        bail!("--repeats must be greater than zero");
    }

    // Vocabulary and input loading are deliberately outside measured
    // projection time.
    let target_vocab = TargetVocab::load(&args.target_model_bin)?;

    let raw_all =
        fs::read(&args.raw).with_context(|| format!("read raw input {}", args.raw.display()))?;

    let input_len = if args.max_bytes == 0 {
        raw_all.len()
    } else {
        args.max_bytes.min(raw_all.len())
    };

    if input_len == 0 {
        bail!("input is empty");
    }

    let input = &raw_all[..input_len];

    let (_, model_sha256) = hash_file(&args.target_model_bin)?;
    let input_sha256 = sha256_hex(input);

    // Warm-up uses the exact same path but is intentionally excluded from
    // measured timings.
    for _ in 0..args.warmups {
        let ids = target_vocab.greedy_encode(input)?;
        std::hint::black_box(ids);
    }

    let mut wall_ns = Vec::with_capacity(args.repeats);
    let mut last_ids = Vec::new();
    let mut expected_count = None;

    for _ in 0..args.repeats {
        let started = Instant::now();
        let ids = target_vocab.greedy_encode(input)?;
        let elapsed = started.elapsed().as_nanos();

        if let Some(expected) = expected_count {
            if ids.len() != expected {
                bail!(
                    "non-deterministic ID count across repeats: {} != {}",
                    ids.len(),
                    expected
                );
            }
        } else {
            expected_count = Some(ids.len());
        }

        wall_ns.push(elapsed);
        last_ids = ids;
    }

    // Exact reconstruction validation is deliberately outside the measured
    // encode timing.
    let decoded = target_vocab.decode(&last_ids)?;
    let decoded_sha256 = sha256_hex(&decoded);
    let exact = decoded == input;

    if !exact {
        bail!("target-vocabulary projection changed source bytes");
    }

    if let Some(path) = &args.id_output {
        write_ids(path, &last_ids)?;
    }

    let min_wall_ns = *wall_ns.iter().min().unwrap();
    let max_wall_ns = *wall_ns.iter().max().unwrap();
    let median_wall_ns = median(&wall_ns);
    let mean_wall_ns = wall_ns.iter().map(|v| *v as f64).sum::<f64>() / wall_ns.len() as f64;

    let median_seconds = median_wall_ns as f64 / 1_000_000_000.0;
    let median_bytes_per_sec = input.len() as f64 / median_seconds;

    let receipt = Receipt {
        schema: "nereids-target-greedy-bench-v1",
        mode: "continuous longest-byte-match target-vocabulary projection",
        claim: "Native target-vocabulary IDs reconstruct exactly the same input bytes.",
        not_claimed: "Canonical target-tokenizer segmentation or model-state equivalence.",

        raw: args.raw.display().to_string(),
        input_bytes: input.len(),
        input_sha256,

        target_model_bin: args.target_model_bin.display().to_string(),
        target_model_bin_sha256: model_sha256,
        target_vocab_entries: target_vocab.entries(),
        target_vocab_max_token_bytes: target_vocab.max_token_bytes,

        warmups: args.warmups,
        repeats: args.repeats,
        wall_ns,
        min_wall_ns,
        median_wall_ns,
        mean_wall_ns,
        max_wall_ns,

        median_bytes_per_sec,
        median_mb_per_sec_decimal: median_bytes_per_sec / 1_000_000.0,

        emitted_target_ids: last_ids.len(),
        bytes_per_target_id: input.len() as f64 / last_ids.len() as f64,
        target_ids_sha256_u32le: ids_sha256(&last_ids),

        decoded_bytes: decoded.len(),
        decoded_sha256,
        exact_byte_reconstruction: exact,

        peak_rss_kib: peak_rss_kib(),
        id_output: args.id_output.as_ref().map(|p| p.display().to_string()),
    };

    if let Some(parent) = args.receipt.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    fs::write(
        &args.receipt,
        serde_json::to_string_pretty(&receipt)? + "\n",
    )
    .with_context(|| format!("write {}", args.receipt.display()))?;

    println!("{}", serde_json::to_string_pretty(&receipt)?);

    Ok(())
}
