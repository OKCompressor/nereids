use anyhow::{Context, Result};
use clap::Parser;
use nereids::{align_token_spans, import_native_token_jsonl, sha256_hex, TokenSpanAlignment};
use serde_json::json;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Parser)]
#[command(
    name = "nereids-bench",
    about = "Receipt-oriented Nereids native-token span benchmark"
)]
struct Args {
    /// Canonical raw byte stream.
    #[arg(long)]
    raw: PathBuf,

    /// Source native-token JSONL generated outside the timed region.
    #[arg(long)]
    source: PathBuf,

    /// Target native-token JSONL generated outside the timed region.
    #[arg(long)]
    target: PathBuf,

    #[arg(long, default_value = "source")]
    source_name: String,

    #[arg(long, default_value = "external")]
    source_provenance: String,

    #[arg(long, default_value = "target")]
    target_name: String,

    #[arg(long, default_value = "external")]
    target_provenance: String,

    /// Measured repetitions per direction.
    #[arg(long, default_value_t = 9)]
    reps: usize,

    /// Unmeasured warmup repetitions per direction.
    #[arg(long, default_value_t = 2)]
    warmup: usize,

    /// Optional JSON receipt path. JSON is always printed to stdout.
    #[arg(long)]
    output: Option<PathBuf>,
}

fn ns_u64(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn median(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();

    if sorted.is_empty() {
        return 0;
    }

    sorted[sorted.len() / 2]
}

fn mib_per_sec(bytes: usize, nanos: u64) -> f64 {
    if nanos == 0 {
        return 0.0;
    }

    let seconds = nanos as f64 / 1_000_000_000.0;
    bytes as f64 / (1024.0 * 1024.0) / seconds
}

fn alignment_stats(items: &[TokenSpanAlignment]) -> serde_json::Value {
    let exact = items
        .iter()
        .filter(|item| item.starts_on_right_boundary && item.ends_on_right_boundary)
        .count();

    let start_inside = items
        .iter()
        .filter(|item| !item.starts_on_right_boundary)
        .count();

    let end_inside = items
        .iter()
        .filter(|item| !item.ends_on_right_boundary)
        .count();

    let max_right_tokens = items
        .iter()
        .map(|item| item.right_token_count)
        .max()
        .unwrap_or(0);

    json!({
        "left_tokens": items.len(),
        "exact_on_right_boundaries": exact,
        "cross_or_cut": items.len().saturating_sub(exact),
        "start_inside_right_token": start_inside,
        "end_inside_right_token": end_inside,
        "max_right_tokens_covered": max_right_tokens
    })
}

fn run_direction(
    raw_len: usize,
    warmup: usize,
    reps: usize,
    left: &nereids::TokenizerStream,
    right: &nereids::TokenizerStream,
) -> Result<serde_json::Value> {
    for _ in 0..warmup {
        let aligned = align_token_spans(left, right)?;
        black_box(aligned);
    }

    let mut times_ns = Vec::with_capacity(reps);
    let mut final_alignment = Vec::new();

    for rep in 0..reps {
        let started = Instant::now();
        let aligned = align_token_spans(left, right)?;
        let elapsed = ns_u64(started);

        black_box(&aligned);
        times_ns.push(elapsed);

        if rep + 1 == reps {
            final_alignment = aligned;
        }
    }

    let min_ns = times_ns.iter().copied().min().unwrap_or(0);
    let max_ns = times_ns.iter().copied().max().unwrap_or(0);
    let median_ns = median(&times_ns);

    Ok(json!({
        "warmup": warmup,
        "repetitions": reps,
        "samples_ns": times_ns,
        "min_ns": min_ns,
        "median_ns": median_ns,
        "max_ns": max_ns,
        "median_mib_per_sec": mib_per_sec(raw_len, median_ns),
        "alignment": alignment_stats(&final_alignment)
    }))
}

fn file_sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

fn linux_vmhwm_kib() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;

    status.lines().find_map(|line| {
        let rest = line.strip_prefix("VmHWM:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.reps == 0 {
        anyhow::bail!("--reps must be greater than zero");
    }

    let raw =
        fs::read(&args.raw).with_context(|| format!("read raw input {}", args.raw.display()))?;

    /*
     * Parsing and validation happen before timing. The benchmark measures
     * Nereids span alignment, not Python tokenization, JSON parsing, or I/O.
     */
    let source = import_native_token_jsonl(
        &args.source,
        &args.source_name,
        &args.source_provenance,
        &raw,
    )
    .context("load source token stream")?;

    let target = import_native_token_jsonl(
        &args.target,
        &args.target_name,
        &args.target_provenance,
        &raw,
    )
    .context("load target token stream")?;

    let source_exact = source.reconstruct() == raw;
    let target_exact = target.reconstruct() == raw;

    if !source_exact || !target_exact {
        anyhow::bail!("input streams failed exact reconstruction");
    }

    let source_to_target = run_direction(raw.len(), args.warmup, args.reps, &source, &target)?;

    let target_to_source = run_direction(raw.len(), args.warmup, args.reps, &target, &source)?;

    let receipt = json!({
        "schema": "nereids-span-bench-v1",
        "nereids_version": env!("CARGO_PKG_VERSION"),
        "measurement_scope": "Rust span alignment only; tokenizer encode, JSON parse, validation, and disk I/O excluded",
        "raw": {
            "path": args.raw,
            "bytes": raw.len(),
            "sha256": sha256_hex(&raw)
        },
        "source": {
            "name": args.source_name,
            "provenance": args.source_provenance,
            "token_count": source.tokens.len(),
            "jsonl_sha256": file_sha256(&args.source)?,
            "exact_reconstruction": source_exact
        },
        "target": {
            "name": args.target_name,
            "provenance": args.target_provenance,
            "token_count": target.tokens.len(),
            "jsonl_sha256": file_sha256(&args.target)?,
            "exact_reconstruction": target_exact
        },
        "source_to_target": source_to_target,
        "target_to_source": target_to_source,
        "process_vmhwm_kib_after_run": linux_vmhwm_kib(),
        "memory_note": "VmHWM includes process setup, imported token streams, and benchmark allocations; it is not bridge-only RSS.",
        "exact": source_exact && target_exact
    });

    let pretty = serde_json::to_string_pretty(&receipt)?;

    if let Some(path) = &args.output {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, format!("{pretty}\n"))
            .with_context(|| format!("write {}", path.display()))?;
    }

    println!("{pretty}");
    Ok(())
}
