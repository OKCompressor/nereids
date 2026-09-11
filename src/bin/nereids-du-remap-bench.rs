use anyhow::{bail, Context, Result};
use clap::Parser;
use nereids::{
    hash_file, peak_rss_kib, ratio, translate_du_run, DuRemapReceipt, PrecomputedDuTargetMap,
    TargetVocab,
};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Parser)]
#[command(
    name = "nereids-du-remap-bench",
    about = "Precompute and stream DU-global-ID to target-vocabulary-ID remapping"
)]
struct Args {
    #[arg(long)]
    du_run: PathBuf,
    #[arg(long)]
    target_model_bin: PathBuf,
    #[arg(long)]
    raw: Option<PathBuf>,
    #[arg(long)]
    receipt: PathBuf,
    #[arg(long, default_value_t = 100_000_000)]
    chunk_bytes: u64,
    #[arg(long)]
    id_output: Option<PathBuf>,
    #[arg(long)]
    no_id_output: bool,
}

fn write_u32_output(path: &PathBuf) -> Result<BufWriter<File>> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create ID output directory {}", parent.display()))?;
    }
    Ok(BufWriter::new(File::create(path).with_context(|| {
        format!("create ID output {}", path.display())
    })?))
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.chunk_bytes == 0 {
        bail!("--chunk-bytes must be greater than zero");
    }
    if args.no_id_output && args.id_output.is_some() {
        bail!("--no-id-output cannot be combined with --id-output");
    }

    // Model and dictionary loading are intentionally outside all measured
    // build/remap/validation timers.
    let target_vocab = TargetVocab::load(&args.target_model_bin)?;
    let du_vocab = nereids::load_du_dict(args.du_run.join("merged.dict"))?;

    let map_started = Instant::now();
    let map = PrecomputedDuTargetMap::build(&du_vocab, &target_vocab).with_context(|| {
        format!(
            "build DU-to-target map from {}",
            args.du_run.join("merged.dict").display()
        )
    })?;
    let map_build_wall_ns = map_started.elapsed().as_nanos();

    // Raw hashing is separate and happens before the timed remap path.
    let validation_started = Instant::now();
    let raw_hash = if let Some(raw) = &args.raw {
        Some(hash_file(raw).with_context(|| format!("hash raw input {}", raw.display()))?)
    } else {
        None
    };
    let validation_wall_ns = args
        .raw
        .as_ref()
        .map(|_| validation_started.elapsed().as_nanos());

    let mut id_writer = match &args.id_output {
        Some(path) => Some(write_u32_output(path)?),
        None => None,
    };
    let remap_started = Instant::now();
    let summary = translate_du_run(
        &args.du_run,
        &du_vocab,
        &map,
        &target_vocab,
        args.chunk_bytes,
        id_writer
            .as_mut()
            .map(|writer| writer as &mut BufWriter<File>),
    )?;
    if let Some(writer) = id_writer.as_mut() {
        std::io::Write::flush(writer)?;
    }
    let remap_wall_ns = remap_started.elapsed().as_nanos();

    let raw_sha256 = raw_hash.as_ref().map(|(_, hash)| hash.clone());
    let raw_length_match = raw_hash
        .as_ref()
        .map(|(bytes, _)| *bytes == summary.du_bytes)
        .unwrap_or(true);
    let raw_hash_match = raw_sha256
        .as_ref()
        .map(|hash| hash == &summary.du_lexeme_sha256)
        .unwrap_or(true);
    let hash_match = summary.du_lexeme_sha256 == summary.emitted_target_bytes_sha256
        && raw_length_match
        && raw_hash_match;
    let input_bytes = raw_hash
        .as_ref()
        .map(|(bytes, _)| *bytes)
        .unwrap_or(summary.du_bytes);
    let bytes_per_sec = (remap_wall_ns > 0)
        .then_some(summary.du_bytes as f64 / (remap_wall_ns as f64 / 1_000_000_000.0));
    let map_bytes = map.map_bytes();
    let receipt = DuRemapReceipt {
        mode: "precomputed DU-global-ID to target-vocabulary-ID remap".to_string(),
        claim: "DU-derived target-vocabulary IDs decode to exactly the same bytes.".to_string(),
        not_claimed: "IDs equal the target tokenizer's canonical segmentation; drop-in model-tokenizer replacement; native tokenizer equivalence.".to_string(),
        du_run: args.du_run.display().to_string(),
        target_model_bin: args.target_model_bin.display().to_string(),
        raw: args.raw.as_ref().map(|path| path.display().to_string()),
        du_vocab_entries: du_vocab.len(),
        target_vocab_entries: target_vocab.entries(),
        target_vocab_max_token_bytes: target_vocab.max_token_bytes,
        map_target_ids: map.map_target_ids(),
        map_payload_bytes: map.map_payload_bytes(),
        map_bytes,
        raw_bytes: raw_hash.as_ref().map(|(bytes, _)| *bytes),
        model_bin_bytes: target_vocab.model_bin_bytes,
        map_bytes_per_raw_bytes: ratio(map_bytes, input_bytes),
        map_bytes_per_model_bin_bytes: ratio(map_bytes, target_vocab.model_bin_bytes),
        build_wall_ns: map_build_wall_ns,
        map_build_wall_ns,
        remap_wall_ns,
        validation_wall_ns,
        unmappable_du_entries: 0,
        exact_dictionary_decode: true,
        du_positions_consumed: summary.du_positions_consumed,
        emitted_target_ids: summary.emitted_target_ids,
        du_bytes: summary.du_bytes,
        target_decoded_bytes: summary.target_decoded_bytes,
        du_lexeme_sha256: summary.du_lexeme_sha256,
        emitted_target_bytes_sha256: summary.emitted_target_bytes_sha256,
        raw_sha256,
        all_hashes_exact: hash_match,
        exact_hash_match: hash_match,
        chunk_target_bytes: summary.chunk_target_bytes,
        chunks_processed: summary.chunks_processed,
        input_bytes,
        peak_rss_kib: peak_rss_kib(),
        bytes_per_sec,
        map_bytes_per_du_entry: ratio(map_bytes, du_vocab.len() as u64),
        id_output: args.id_output.as_ref().map(|path| path.display().to_string()),
    };

    if let Some(parent) = args
        .receipt
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create receipt directory {}", parent.display()))?;
    }
    fs::write(
        &args.receipt,
        serde_json::to_string_pretty(&receipt)? + "\n",
    )
    .with_context(|| format!("write receipt {}", args.receipt.display()))?;

    println!("{}", serde_json::to_string_pretty(&receipt)?);
    if !hash_match {
        bail!("DU and emitted target bytes, or optional raw bytes, do not hash identically");
    }
    Ok(())
}
