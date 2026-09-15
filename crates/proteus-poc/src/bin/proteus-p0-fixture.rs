use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use nereids::{import_native_token_jsonl, load_du_stream, sha256_hex, TokenSpan};
use proteus_poc::{
    persist_snapshot, persist_snapshot_bytes, resume_snapshot_json,
    verify_with_full_tokenize_oracle, ModelTokenizerIdentityV0, NativeTokenPieceV0,
    ProteusResumeTimingsV0, ProteusSnapshotV0, SnapshotProvenanceEventV0,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "proteus-p0-fixture")]
#[command(about = "Exact rendered-byte snapshot/delta fixture over Nereids Direct-ID V2")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize sequence zero from exact prefix bytes and persisted streams.
    Init(InitArgs),
    /// Build an incremental candidate, then consult the full-tokenization oracle.
    Append(AppendArgs),
    /// Deserialize and fully validate a persisted snapshot.
    Validate(ValidateArgs),
}

#[derive(Debug, clap::Args)]
struct InitArgs {
    #[arg(long)]
    prefix: PathBuf,
    #[arg(long)]
    prefix_du_run: PathBuf,
    #[arg(long)]
    native_spans: PathBuf,
    #[arg(long)]
    model_id: String,
    #[arg(long)]
    model_provenance: String,
    #[arg(long)]
    tokenizer_id: String,
    #[arg(long)]
    tokenizer_provenance: String,
    /// Stable lineage label shared by this snapshot and all of its appends.
    #[arg(long)]
    dictionary_lineage: String,
    #[arg(long)]
    actor: String,
    #[arg(long)]
    source: String,
    #[arg(long)]
    snapshot: PathBuf,
}

#[derive(Debug, clap::Args)]
struct AppendArgs {
    #[arg(long)]
    snapshot: PathBuf,
    #[arg(long)]
    delta: PathBuf,
    #[arg(long)]
    delta_du_run: PathBuf,
    /// Fixed number of native prefix tokens replaced by repair-window IDs.
    #[arg(long)]
    drop_tokens: usize,
    #[arg(long)]
    native_tokenizer_server: String,
    #[arg(long)]
    actor: String,
    #[arg(long)]
    source: String,
    /// May equal --snapshot for an atomic in-place update. A distinct path
    /// preserves the previous snapshot as an explicit rollback point.
    #[arg(long)]
    output_snapshot: PathBuf,
    #[arg(long)]
    receipt: PathBuf,
}

#[derive(Debug, clap::Args)]
struct ValidateArgs {
    #[arg(long)]
    snapshot: PathBuf,
}

#[derive(Debug, Serialize)]
struct AppendReceipt {
    schema: &'static str,
    model_tokenizer: ModelTokenizerIdentityV0,
    dictionary_lineage: String,
    previous_append_sequence: u64,
    append_sequence: u64,
    transcript_len: usize,
    transcript_sha256: String,
    delta_len: usize,
    delta_sha256: String,
    fixed_repair_depth: usize,
    kept_snapshot_token_count: usize,
    repair_start_byte: usize,
    repair_window_bytes: usize,
    repair_token_count: usize,
    candidate_token_count: usize,
    oracle_token_count: usize,
    candidate_ids_sha256_u32le: String,
    oracle_ids_sha256_u32le: String,
    ids_exact: bool,
    repair_selection_mode: &'static str,
    candidate_constructed_before_oracle: bool,
    oracle_used_for_candidate_construction: bool,
    oracle_used_for_drop_selection: bool,
    oracle_used_for_verification: bool,
    snapshot_persisted: bool,
    new_dictionary_entries: usize,
    timings: ProteusResumeTimingsV0,
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
        concat!(
            "POST {path} HTTP/1.1\r\n",
            "Host: {host}:{port}\r\n",
            "Content-Type: application/json\r\n",
            "Content-Length: {content_len}\r\n",
            "Connection: close\r\n\r\n",
            "{body}"
        ),
        path = path,
        host = host,
        port = port,
        content_len = body.len(),
        body = body,
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

fn tokenize_response(server: &str, bytes: &[u8], with_pieces: bool) -> Result<Value> {
    let content = std::str::from_utf8(bytes)
        .context("fixture tokenizer input must be valid UTF-8 rendered conversation bytes")?;
    let body = serde_json::to_string(&json!({
        "content": content,
        "add_special": false,
        "with_pieces": with_pieces
    }))?;
    let response = http_post_json(server, "/tokenize", &body)?;
    serde_json::from_str(&response).context("parse /tokenize response JSON")
}

fn tokens_array(value: &Value) -> Result<&Vec<Value>> {
    value
        .get("tokens")
        .or_else(|| value.get("ids"))
        .and_then(Value::as_array)
        .context("/tokenize response has no tokens or ids array")
}

fn parse_token_id(value: &Value) -> Result<u32> {
    let raw = value
        .as_u64()
        .or_else(|| value.get("id").and_then(Value::as_u64))
        .context("token is neither an unsigned ID nor an object with an unsigned id")?;
    u32::try_from(raw).context("token ID exceeds u32")
}

fn parse_piece_bytes(value: &Value) -> Result<Vec<u8>> {
    let piece = value
        .get("piece")
        .or_else(|| value.get("bytes"))
        .context("with_pieces token object has no piece or bytes field")?;
    if let Some(text) = piece.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    if let Some(values) = piece.as_array() {
        return values
            .iter()
            .map(|value| {
                let byte = value.as_u64().context("token piece byte is not unsigned")?;
                u8::try_from(byte).context("token piece byte exceeds u8")
            })
            .collect();
    }
    bail!("token piece must be a string or byte array")
}

fn tokenize_with_pieces(server: &str, bytes: &[u8]) -> Result<Vec<NativeTokenPieceV0>> {
    let value = tokenize_response(server, bytes, true)?;
    tokens_array(&value)?
        .iter()
        .map(|value| {
            Ok(NativeTokenPieceV0 {
                id: parse_token_id(value)?,
                bytes: parse_piece_bytes(value)?,
            })
        })
        .collect()
}

fn full_tokenize(server: &str, bytes: &[u8]) -> Result<Vec<u32>> {
    let value = tokenize_response(server, bytes, false)?;
    tokens_array(&value)?.iter().map(parse_token_id).collect()
}

fn sha256_ids_u32le(ids: &[u32]) -> String {
    let mut bytes = Vec::with_capacity(ids.len() * std::mem::size_of::<u32>());
    for id in ids {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    sha256_hex(&bytes)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create receipt directory {}", parent.display()))?;
    }
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("write receipt {}", path.display()))
}

fn init(args: InitArgs) -> Result<()> {
    let prefix =
        fs::read(&args.prefix).with_context(|| format!("read prefix {}", args.prefix.display()))?;
    let (prefix_du, _) = load_du_stream(&args.prefix_du_run, usize::MAX)
        .with_context(|| format!("load prefix DU run {}", args.prefix_du_run.display()))?;
    let native_stream = import_native_token_jsonl(
        &args.native_spans,
        &args.tokenizer_id,
        &args.tokenizer_provenance,
        &prefix,
    )
    .with_context(|| format!("load native spans {}", args.native_spans.display()))?;
    let native_ids = native_stream
        .tokens
        .iter()
        .map(|token| u32::try_from(token.id).context("native token ID exceeds u32"))
        .collect::<Result<Vec<_>>>()?;
    let native_spans = native_stream
        .tokens
        .iter()
        .map(|token| TokenSpan {
            id: token.id,
            byte_start: token.byte_start,
            byte_end: token.byte_end,
            bytes_hex: nereids::bytes_to_hex(&token.bytes),
        })
        .collect();
    let snapshot = ProteusSnapshotV0::initialize(
        &prefix,
        native_ids,
        native_spans,
        prefix_du,
        args.dictionary_lineage,
        ModelTokenizerIdentityV0 {
            model_id: args.model_id,
            model_provenance: args.model_provenance,
            tokenizer_id: args.tokenizer_id,
            tokenizer_provenance: args.tokenizer_provenance,
        },
        SnapshotProvenanceEventV0::now(0, args.actor, args.source),
    )?;
    persist_snapshot(&args.snapshot, &snapshot)?;
    println!("schema={}", snapshot.schema);
    println!("append_sequence={}", snapshot.append_sequence);
    println!("dictionary_lineage={}", snapshot.dictionary_lineage);
    println!("transcript_len={}", snapshot.transcript_len);
    println!("transcript_sha256={}", snapshot.transcript_sha256);
    println!("snapshot={}", args.snapshot.display());
    Ok(())
}

fn append(args: AppendArgs) -> Result<()> {
    if args.receipt == args.snapshot || args.receipt == args.output_snapshot {
        bail!("--receipt must be distinct from both snapshot paths");
    }
    let snapshot_json = fs::read(&args.snapshot)
        .with_context(|| format!("read snapshot {}", args.snapshot.display()))?;
    // This unvalidated metadata peek exists only to label the new provenance
    // event. The timed resume path independently deserializes and validates the
    // complete snapshot before any candidate work.
    let prior_value: Value = serde_json::from_slice(&snapshot_json)
        .context("read append sequence for provenance labeling")?;
    let previous_sequence = prior_value
        .get("append_sequence")
        .and_then(Value::as_u64)
        .context("snapshot has no unsigned append_sequence")?;
    let next_sequence = previous_sequence
        .checked_add(1)
        .context("snapshot append sequence overflow")?;
    let delta =
        fs::read(&args.delta).with_context(|| format!("read delta {}", args.delta.display()))?;
    let (delta_du, _) = load_du_stream(&args.delta_du_run, usize::MAX)
        .with_context(|| format!("load delta DU run {}", args.delta_du_run.display()))?;

    let server = args.native_tokenizer_server.clone();
    let mut repair_tokenizer = |bytes: &[u8]| tokenize_with_pieces(&server, bytes);
    // This returns only after the candidate and serialized next snapshot are
    // complete. No oracle IDs are accepted anywhere in this call.
    let artifact = resume_snapshot_json(
        &snapshot_json,
        &delta,
        &delta_du,
        args.drop_tokens,
        SnapshotProvenanceEventV0::now(next_sequence, args.actor, args.source),
        &mut repair_tokenizer,
    )?;

    let server = args.native_tokenizer_server.clone();
    let mut oracle = |bytes: &[u8]| full_tokenize(&server, bytes);
    let verified = verify_with_full_tokenize_oracle(artifact, &mut oracle)?;
    let mut receipt = AppendReceipt {
        schema: "proteus-p0-append-receipt-v0",
        model_tokenizer: verified.artifact.updated_snapshot.model_tokenizer.clone(),
        dictionary_lineage: verified
            .artifact
            .updated_snapshot
            .dictionary_lineage
            .clone(),
        previous_append_sequence: verified.artifact.previous_append_sequence,
        append_sequence: verified.artifact.updated_snapshot.append_sequence,
        transcript_len: verified.artifact.updated_snapshot.transcript_len,
        transcript_sha256: verified.artifact.updated_snapshot.transcript_sha256.clone(),
        delta_len: delta.len(),
        delta_sha256: sha256_hex(&delta),
        fixed_repair_depth: verified.artifact.candidate.fixed_repair_depth,
        kept_snapshot_token_count: verified.artifact.candidate.kept_snapshot_token_count,
        repair_start_byte: verified.artifact.candidate.repair_start_byte,
        repair_window_bytes: verified.artifact.candidate.repair_window_bytes,
        repair_token_count: verified.artifact.candidate.repair_token_count,
        candidate_token_count: verified.artifact.candidate.ids.len(),
        oracle_token_count: verified.oracle_ids.len(),
        candidate_ids_sha256_u32le: sha256_ids_u32le(&verified.artifact.candidate.ids),
        oracle_ids_sha256_u32le: sha256_ids_u32le(&verified.oracle_ids),
        ids_exact: verified.ids_exact,
        repair_selection_mode: "fixed_depth",
        candidate_constructed_before_oracle: true,
        oracle_used_for_candidate_construction: false,
        oracle_used_for_drop_selection: false,
        oracle_used_for_verification: true,
        snapshot_persisted: false,
        new_dictionary_entries: verified.artifact.new_dictionary_entries,
        timings: verified.artifact.timings.clone(),
    };
    if !verified.ids_exact {
        write_json(&args.receipt, &receipt)?;
        bail!("incremental candidate differs from oracle; next snapshot was not installed");
    }
    persist_snapshot_bytes(
        &args.output_snapshot,
        &verified.artifact.serialized_snapshot,
    )?;
    receipt.snapshot_persisted = true;
    write_json(&args.receipt, &receipt)?;

    println!("schema={}", receipt.schema);
    println!("append_sequence={}", receipt.append_sequence);
    println!("dictionary_lineage={}", receipt.dictionary_lineage);
    println!("candidate_constructed_before_oracle=true");
    println!("oracle_used_for_candidate_construction=false");
    println!("ids_exact={}", receipt.ids_exact);
    println!("snapshot_persisted={}", receipt.snapshot_persisted);
    println!(
        "proteus_resume_total_wall_ns={}",
        receipt.timings.proteus_resume_total_wall_ns
    );
    println!(
        "oracle_full_tokenize_wall_ns={}",
        receipt.timings.oracle_full_tokenize_wall_ns
    );
    println!("output_snapshot={}", args.output_snapshot.display());
    println!("receipt={}", args.receipt.display());
    Ok(())
}

fn validate(args: ValidateArgs) -> Result<()> {
    let snapshot = proteus_poc::load_snapshot(&args.snapshot)?;
    println!("schema={}", snapshot.schema);
    println!("append_sequence={}", snapshot.append_sequence);
    println!("dictionary_lineage={}", snapshot.dictionary_lineage);
    println!("transcript_len={}", snapshot.transcript_len);
    println!("transcript_sha256={}", snapshot.transcript_sha256);
    println!("valid=true");
    Ok(())
}

fn run(args: Args) -> Result<()> {
    match args.command {
        Command::Init(args) => init(args),
        Command::Append(args) => append(args),
        Command::Validate(args) => validate(args),
    }
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("FAIL: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_integer_and_piece_token_shapes() {
        assert_eq!(parse_token_id(&json!(42)).unwrap(), 42);
        assert_eq!(parse_token_id(&json!({"id": 7, "piece": "hi"})).unwrap(), 7);
        assert_eq!(
            parse_piece_bytes(&json!({"id": 7, "piece": "hi"})).unwrap(),
            b"hi"
        );
        assert_eq!(
            parse_piece_bytes(&json!({"id": 7, "bytes": [0, 255]})).unwrap(),
            [0, 255]
        );
    }
}
