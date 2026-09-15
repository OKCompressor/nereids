use anyhow::{bail, Context, Result};
use clap::Parser;
use nereids::direct_id::{search_exact_repair, ExactRepair, RepairTrial};
use nereids::sha256_hex;
use serde::Serialize;
use serde_json::{json, Value};
use std::cell::Cell;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Parser)]
#[command(name = "nereids-direct-id-repair-fixture")]
#[command(about = "Oracle-verified bounded append repair for native tokenizer IDs")]
struct Args {
    #[arg(long)]
    prefix: PathBuf,

    #[arg(long)]
    delta: PathBuf,

    #[arg(long)]
    native_tokenizer_server: String,

    #[arg(long)]
    model_name: String,

    #[arg(long, default_value_t = 32)]
    max_drop_tokens: usize,

    #[arg(long)]
    receipt: PathBuf,
}

#[derive(Debug)]
struct TimedTokens {
    ids: Vec<u32>,
    wall_ns: u128,
}

#[derive(Debug, Serialize)]
struct Receipt {
    schema: &'static str,
    model_name: String,
    prefix_bytes: usize,
    delta_bytes: usize,
    full_bytes: usize,
    prefix_sha256: String,
    delta_sha256: String,
    full_sha256: String,
    prefix_token_count: usize,
    oracle_token_count: usize,
    candidate_token_count: usize,
    minimum_exact_drop_tokens: usize,
    snapshot_tail_bytes_repaired: usize,
    repair_window_bytes: usize,
    repair_token_count: usize,
    candidate_ids_sha256_u32le: String,
    oracle_ids_sha256_u32le: String,
    ids_exact: bool,
    decode_exact: bool,
    prefix_tokenize_wall_ns: u128,
    candidate_repair_tokenize_wall_ns: u128,
    oracle_full_tokenize_wall_ns: u128,
    candidate_used_full_window: bool,
    oracle_used_for_verification_only: bool,
    trials: Vec<RepairTrial>,
}

fn parse_http_base(base: &str) -> Result<(String, u16)> {
    let base = base.trim().trim_end_matches('/');
    let authority = base
        .strip_prefix("http://")
        .context("only http:// tokenizer server URLs are supported")?;
    let (host, port) = authority
        .rsplit_once(':')
        .context("tokenizer server URL must include a port")?;
    if host.is_empty() || port.is_empty() || host.contains('/') {
        bail!("tokenizer server URL must have the form http://host:port");
    }
    Ok((
        host.to_owned(),
        port.parse().context("invalid tokenizer server port")?,
    ))
}

fn http_post_json(base: &str, path: &str, body: &str) -> Result<String> {
    let (host, port) = parse_http_base(base)?;
    let mut stream = TcpStream::connect((host.as_str(), port))
        .with_context(|| format!("connect to tokenizer server {host}:{port}"))?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .context("write tokenizer request")?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("read tokenizer response")?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .context("malformed HTTP response from tokenizer server")?;
    let status = head.lines().next().unwrap_or_default();
    if !status.contains(" 200 ") && !status.contains(" 201 ") {
        bail!("tokenizer server returned {status}: {body}");
    }
    Ok(body.to_owned())
}

fn tokenize(server: &str, bytes: &[u8]) -> Result<TimedTokens> {
    let content = std::str::from_utf8(bytes).context("tokenizer input is not valid UTF-8")?;
    let body = serde_json::to_string(&json!({
        "content": content,
        "add_special": false
    }))?;
    let started = Instant::now();
    let response = http_post_json(server, "/tokenize", &body)?;
    let wall_ns = started.elapsed().as_nanos();
    let value: Value = serde_json::from_str(&response).context("parse /tokenize response JSON")?;
    let values = value
        .get("tokens")
        .or_else(|| value.get("ids"))
        .and_then(Value::as_array)
        .context("/tokenize response has no tokens or ids array")?;
    let ids = values
        .iter()
        .map(|value| {
            let id = value
                .as_u64()
                .context("token ID is not an unsigned integer")?;
            u32::try_from(id).context("token ID exceeds u32")
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(TimedTokens { ids, wall_ns })
}

fn detokenize(server: &str, ids: &[u32]) -> Result<Vec<u8>> {
    let body = serde_json::to_string(&json!({
        "tokens": ids,
        "special": false
    }))?;
    let response = http_post_json(server, "/detokenize", &body)?;
    let value: Value =
        serde_json::from_str(&response).context("parse /detokenize response JSON")?;
    let content = value
        .get("content")
        .or_else(|| value.get("text"))
        .and_then(Value::as_str)
        .context("/detokenize response has no content or text string")?;
    Ok(content.as_bytes().to_vec())
}

fn sha256_ids_u32le(ids: &[u32]) -> String {
    let mut bytes = Vec::with_capacity(ids.len() * std::mem::size_of::<u32>());
    for id in ids {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    sha256_hex(&bytes)
}

fn write_receipt(path: &Path, receipt: &Receipt) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create receipt directory {}", parent.display()))?;
    }
    let mut json = serde_json::to_vec_pretty(receipt)?;
    json.push(b'\n');
    fs::write(path, json).with_context(|| format!("write receipt {}", path.display()))
}

fn verify_candidate(
    prefix: &[u8],
    delta: &[u8],
    prefix_token_count: usize,
    oracle: &TimedTokens,
    repair: ExactRepair,
    candidate_repair_tokenize_wall_ns: u128,
    prefix_tokenize_wall_ns: u128,
    server: &str,
    model_name: String,
) -> Result<Receipt> {
    let mut full = Vec::with_capacity(prefix.len() + delta.len());
    full.extend_from_slice(prefix);
    full.extend_from_slice(delta);

    let candidate = repair.candidate;
    let ids_exact = candidate.ids == oracle.ids;
    let decoded = detokenize(server, &candidate.ids).context("verify candidate decode")?;
    let decode_exact = decoded == full;
    let candidate_used_full_window = candidate.kept_byte_len == 0;
    let oracle_used_for_verification_only = true;

    if candidate_used_full_window {
        bail!("candidate construction used the prohibited full prefix-plus-delta window");
    }
    if !ids_exact {
        bail!("candidate IDs differ from the full-tokenization oracle");
    }
    if !decode_exact {
        bail!("candidate IDs do not decode exactly to prefix plus delta");
    }

    Ok(Receipt {
        schema: "nereids-direct-id-repair-fixture-v1",
        model_name,
        prefix_bytes: prefix.len(),
        delta_bytes: delta.len(),
        full_bytes: full.len(),
        prefix_sha256: sha256_hex(prefix),
        delta_sha256: sha256_hex(delta),
        full_sha256: sha256_hex(&full),
        prefix_token_count,
        oracle_token_count: oracle.ids.len(),
        candidate_token_count: candidate.ids.len(),
        minimum_exact_drop_tokens: candidate.drop_tokens,
        snapshot_tail_bytes_repaired: candidate.snapshot_tail_bytes_repaired,
        repair_window_bytes: candidate.repair_window_bytes,
        repair_token_count: candidate.repair_token_count,
        candidate_ids_sha256_u32le: sha256_ids_u32le(&candidate.ids),
        oracle_ids_sha256_u32le: sha256_ids_u32le(&oracle.ids),
        ids_exact,
        decode_exact,
        prefix_tokenize_wall_ns,
        candidate_repair_tokenize_wall_ns,
        oracle_full_tokenize_wall_ns: oracle.wall_ns,
        candidate_used_full_window,
        oracle_used_for_verification_only,
        trials: repair.trials,
    })
}

fn run(args: Args) -> Result<()> {
    let prefix =
        fs::read(&args.prefix).with_context(|| format!("read prefix {}", args.prefix.display()))?;
    let delta =
        fs::read(&args.delta).with_context(|| format!("read delta {}", args.delta.display()))?;
    if prefix.is_empty() {
        bail!(
            "prefix must be nonempty so candidate construction cannot become a full-window fallback"
        );
    }

    let prefix_tokens = tokenize(&args.native_tokenizer_server, &prefix)
        .context("native-tokenize prefix snapshot")?;

    // This is the verification oracle. Its result is passed only to the
    // equality check in search_exact_repair, never to candidate construction.
    let mut full = Vec::with_capacity(prefix.len() + delta.len());
    full.extend_from_slice(&prefix);
    full.extend_from_slice(&delta);
    let oracle = tokenize(&args.native_tokenizer_server, &full)
        .context("native-tokenize full verification oracle")?;

    let candidate_repair_tokenize_wall_ns = Cell::new(0u128);
    let mut tokenize_repair_window = |bytes: &[u8]| {
        let result =
            tokenize(&args.native_tokenizer_server, bytes).map_err(|error| error.to_string())?;
        candidate_repair_tokenize_wall_ns.set(
            candidate_repair_tokenize_wall_ns
                .get()
                .saturating_add(result.wall_ns),
        );
        Ok(result.ids)
    };
    let mut detokenize_kept_ids = |ids: &[u32]| {
        detokenize(&args.native_tokenizer_server, ids).map_err(|error| error.to_string())
    };

    let repair = search_exact_repair(
        &prefix,
        &delta,
        &prefix_tokens.ids,
        &oracle.ids,
        args.max_drop_tokens,
        &mut tokenize_repair_window,
        &mut detokenize_kept_ids,
    )
    .map_err(anyhow::Error::new)?;

    let receipt = verify_candidate(
        &prefix,
        &delta,
        prefix_tokens.ids.len(),
        &oracle,
        repair,
        candidate_repair_tokenize_wall_ns.get(),
        prefix_tokens.wall_ns,
        &args.native_tokenizer_server,
        args.model_name,
    )?;
    write_receipt(&args.receipt, &receipt)?;

    println!("schema={}", receipt.schema);
    println!("model_name={}", receipt.model_name);
    println!("prefix_token_count={}", receipt.prefix_token_count);
    println!("oracle_token_count={}", receipt.oracle_token_count);
    println!("candidate_token_count={}", receipt.candidate_token_count);
    println!(
        "minimum_exact_drop_tokens={}",
        receipt.minimum_exact_drop_tokens
    );
    println!(
        "snapshot_tail_bytes_repaired={}",
        receipt.snapshot_tail_bytes_repaired
    );
    println!("repair_window_bytes={}", receipt.repair_window_bytes);
    println!("ids_exact={}", receipt.ids_exact);
    println!("decode_exact={}", receipt.decode_exact);
    println!(
        "candidate_used_full_window={}",
        receipt.candidate_used_full_window
    );
    println!(
        "oracle_used_for_verification_only={}",
        receipt.oracle_used_for_verification_only
    );
    println!(
        "candidate_repair_tokenize_wall_ns={}",
        receipt.candidate_repair_tokenize_wall_ns
    );
    println!(
        "oracle_full_tokenize_wall_ns={}",
        receipt.oracle_full_tokenize_wall_ns
    );
    println!("receipt={}", args.receipt.display());
    Ok(())
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("FAIL: {error:#}");
        std::process::exit(1);
    }
}
