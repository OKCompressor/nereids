use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "nereids")]
#[command(version)]
#[command(about = "Nereids DU/BPE structural bridge module")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Doctor,
    Hex {
        input: PathBuf,
    },
    Smoke {
        input: PathBuf,
        #[arg(long, default_value = "receipts")]
        receipt_dir: PathBuf,
    },
    LexemeDecode {
        lexeme: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, default_value = "receipts")]
        receipt_dir: PathBuf,
    },
    SpanLedger {
        #[arg(long)]
        du_run: PathBuf,
        #[arg(long)]
        raw: PathBuf,
        #[arg(long)]
        tik_remap: PathBuf,
        #[arg(long, default_value = "receipts")]
        out: PathBuf,
        #[arg(long, default_value_t = 2_000_000)]
        max_bytes: usize,
    },
    BridgeJsonl {
        raw: PathBuf,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        du: PathBuf,
        #[arg(long)]
        target: PathBuf,
        #[arg(long, default_value = "native-source")]
        source_name: String,
        #[arg(long, default_value = "external:native-source")]
        source_provenance: String,
        #[arg(long, default_value = "nereids-du")]
        du_name: String,
        #[arg(long, default_value = "external:nereids-du")]
        du_provenance: String,
        #[arg(long, default_value = "native-target")]
        target_name: String,
        #[arg(long, default_value = "external:native-target")]
        target_provenance: String,
        #[arg(long, default_value = "receipts")]
        receipt_dir: PathBuf,
    },
}

fn write_receipt(dir: &PathBuf, stem: &str, value: &serde_json::Value) -> Result<PathBuf> {
    fs::create_dir_all(dir)
        .with_context(|| format!("create receipt directory {}", dir.display()))?;
    let path = dir.join(format!("{stem}-{}.json", nereids::timestamp()));
    fs::write(&path, serde_json::to_string_pretty(value)? + "\n")
        .with_context(|| format!("write receipt {}", path.display()))?;
    Ok(path)
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Doctor => {
            println!("nereids {}", nereids::version());
            println!("status=ok");
        }
        Cmd::Hex { input } => {
            let data = fs::read(&input)?;
            println!("{}", nereids::bytes_to_hex(&data));
        }
        Cmd::Smoke { input, receipt_dir } => {
            let started = Instant::now();
            let data = fs::read(&input)?;
            let encoded = nereids::bytes_to_hex(&data);
            let rebuilt = nereids::hex_to_bytes(&encoded)?;
            let mismatch = nereids::first_mismatch(&data, &rebuilt);
            nereids::assert_exact_rebuild(&data, &rebuilt)?;

            let report = json!({
                "mode": "byte reconstruction smoke",
                "timestamp": nereids::timestamp(),
                "nereids_version": nereids::version(),
                "input": input,
                "input_size": data.len(),
                "input_sha256": nereids::sha256_hex(&data),
                "wall_time_ms": started.elapsed().as_millis(),
                "exact": mismatch.is_none(),
                "exact_reconstruction": mismatch.is_none(),
                "first_mismatch": mismatch,
            });
            let receipt = write_receipt(&receipt_dir, "smoke", &report)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            eprintln!("receipt={}", receipt.display());
        }
        Cmd::LexemeDecode {
            lexeme,
            file,
            receipt_dir,
        } => {
            if lexeme.is_some() == file.is_some() {
                bail!("provide exactly one lexeme argument or --file");
            }
            let source = if let Some(path) = file {
                fs::read_to_string(&path)
                    .with_context(|| format!("read lexeme file {}", path.display()))?
            } else {
                lexeme.unwrap()
            };
            let decoded = nereids::decode_du_lexeme(&source)?;
            let report = json!({
                "mode": "DU lexeme decode",
                "timestamp": nereids::timestamp(),
                "nereids_version": nereids::version(),
                "input_size": source.len(),
                "input_sha256": nereids::sha256_hex(source.as_bytes()),
                "wall_time_ms": 0,
                "exact": true,
                "exact_reconstruction": true,
                "first_mismatch": serde_json::Value::Null,
                "decoded_size": decoded.len(),
                "decoded_hex": nereids::bytes_to_hex(&decoded),
                "decoded_utf8": String::from_utf8(decoded).ok(),
            });
            let receipt = write_receipt(&receipt_dir, "lexeme-decode", &report)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            eprintln!("receipt={}", receipt.display());
        }
        Cmd::SpanLedger {
            du_run,
            raw,
            tik_remap,
            out,
            max_bytes,
        } => {
            let started = Instant::now();
            let raw_bytes = fs::read(&raw)?;
            let (du, _) = nereids::load_du_stream(&du_run, max_bytes)?;
            let (target, target_vocab_size) = nereids::load_target_stream(&tik_remap, max_bytes)?;
            let report = nereids::span_ledger_report(
                &target,
                &du,
                &raw_bytes,
                target_vocab_size,
                max_bytes,
                started.elapsed().as_millis(),
            );
            let value = serde_json::to_value(&report)?;
            fs::create_dir_all(&out).with_context(|| {
                format!("create span-ledger output directory {}", out.display())
            })?;
            fs::write(
                &out.join("report.json"),
                serde_json::to_string_pretty(&report)? + "\n",
            )
            .with_context(|| format!("write span-ledger report in {}", out.display()))?;
            let receipt = write_receipt(&out, "span-ledger", &value)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            eprintln!("receipt={}", receipt.display());
        }
        Cmd::BridgeJsonl {
            raw,
            source,
            du,
            target,
            source_name,
            source_provenance,
            du_name,
            du_provenance,
            target_name,
            target_provenance,
            receipt_dir,
        } => {
            let started = Instant::now();
            let raw_bytes =
                fs::read(&raw).with_context(|| format!("read bridge input {}", raw.display()))?;
            let source_stream = nereids::import_native_token_jsonl(
                &source,
                source_name,
                source_provenance,
                &raw_bytes,
            )?;
            let du_stream =
                nereids::import_native_token_jsonl(&du, du_name, du_provenance, &raw_bytes)?;
            let target_stream = nereids::import_native_token_jsonl(
                &target,
                target_name,
                target_provenance,
                &raw_bytes,
            )?;
            let bridge = nereids::bridge_tokenizer_streams_timed(
                &raw_bytes,
                &source_stream,
                &du_stream,
                &target_stream,
                Some(started.elapsed().as_millis()),
            )?;
            let report = serde_json::to_value(&bridge.report)?;
            let receipt = write_receipt(&receipt_dir, "bridge-jsonl", &report)?;
            println!("{}", serde_json::to_string_pretty(&bridge.report)?);
            eprintln!("receipt={}", receipt.display());
        }
    }

    Ok(())
}
