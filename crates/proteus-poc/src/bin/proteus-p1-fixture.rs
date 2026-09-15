use anyhow::{bail, Context, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};
use nereids::{load_du_stream, LoadedStream};
use proteus_poc::p1::{
    create_checkpoint, fold_checkpoint, full_audit, hash_hex, inspect, parse_hash_hex, P1Session,
    P1WarmBenchReceipt, P1WarmSession,
};
use proteus_poc::{NativeTokenPieceV0, ProteusSnapshotV0, SnapshotProvenanceEventV0};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Parser)]
#[command(name = "proteus-p1-fixture")]
#[command(about = "Proteus P1 immutable PRT0 checkpoint and append-only PJR0 journal")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// One-time initialization bridge from a fully validated P0 JSON snapshot.
    CheckpointFromP0(CheckpointFromP0Args),
    /// Inspect the accepted checkpoint/journal state as JSON.
    Inspect(StoreArgs),
    /// Construct, oracle-check, and CAS-commit one exact append.
    Append(AppendArgs),
    /// Measure sequential same-process in-memory appends without persistence.
    WarmBench(WarmBenchArgs),
    /// Reconstruct and validate the complete checkpoint/journal lineage.
    FullAudit(StoreArgs),
    /// Fold the accepted state into a new immutable checkpoint.
    Fold(FoldArgs),
}

#[derive(Debug, ClapArgs)]
struct StoreArgs {
    #[arg(long)]
    checkpoint: PathBuf,
    #[arg(long)]
    journal: PathBuf,
}

#[derive(Debug, ClapArgs)]
struct CheckpointFromP0Args {
    #[arg(long)]
    p0_snapshot: PathBuf,
    #[arg(long)]
    checkpoint: PathBuf,
}

#[derive(Debug, ClapArgs)]
struct AppendArgs {
    #[arg(long)]
    checkpoint: PathBuf,
    #[arg(long)]
    journal: PathBuf,
    /// Checkpoint root for the first record, otherwise the prior record root.
    #[arg(long)]
    expected_head: String,
    #[arg(long)]
    delta: PathBuf,
    #[arg(long)]
    delta_du_run: PathBuf,
    #[arg(long)]
    drop_tokens: usize,
    #[arg(long)]
    native_tokenizer_server: String,
    #[arg(long)]
    actor: String,
    #[arg(long)]
    source: String,
    #[arg(long)]
    receipt: PathBuf,
}

#[derive(Debug, ClapArgs)]
struct WarmBenchArgs {
    #[arg(long)]
    checkpoint: PathBuf,
    #[arg(long)]
    journal: PathBuf,
    #[arg(long)]
    expected_head: String,
    #[arg(long)]
    delta: PathBuf,
    #[arg(long)]
    delta_du_run: PathBuf,
    #[arg(long)]
    drop_tokens: usize,
    #[arg(long)]
    native_tokenizer_server: String,
    #[arg(long)]
    iterations: usize,
    #[arg(long)]
    actor: String,
    #[arg(long)]
    source: String,
    #[arg(long)]
    receipt: PathBuf,
}

#[derive(Debug, ClapArgs)]
struct FoldArgs {
    #[arg(long)]
    checkpoint: PathBuf,
    #[arg(long)]
    journal: PathBuf,
    #[arg(long)]
    output_checkpoint: PathBuf,
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
        "POST {path} HTTP/1.1\r\n\
Host: {host}:{port}\r\n\
Content-Type: application/json\r\n\
Content-Length: {content_len}\r\n\
Connection: close\r\n\r\n\
{body}",
        path = path,
        host = host,
        port = port,
        content_len = body.len(),
        body = body,
    );

    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
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
    let request = serde_json::to_string(&json!({
        "content": content,
        "add_special": false,
        "with_pieces": with_pieces,
    }))?;
    serde_json::from_str(&http_post_json(server, "/tokenize", &request)?)
        .context("parse /tokenize response JSON")
}

fn tokens(value: &Value) -> Result<&Vec<Value>> {
    value
        .get("tokens")
        .or_else(|| value.get("ids"))
        .and_then(Value::as_array)
        .context("/tokenize response has no tokens or ids array")
}

fn token_id(value: &Value) -> Result<u32> {
    let id = value
        .as_u64()
        .or_else(|| value.get("id").and_then(Value::as_u64))
        .context("token has no unsigned ID")?;
    u32::try_from(id).context("native token ID exceeds u32")
}

fn piece_bytes(value: &Value) -> Result<Vec<u8>> {
    let piece = value
        .get("piece")
        .or_else(|| value.get("bytes"))
        .context("token object has no piece or bytes field")?;
    if let Some(text) = piece.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    if let Some(bytes) = piece.as_array() {
        return bytes
            .iter()
            .map(|byte| {
                let byte = byte.as_u64().context("token piece byte is not unsigned")?;
                u8::try_from(byte).context("token piece byte exceeds u8")
            })
            .collect();
    }
    bail!("token piece must be a UTF-8 string or byte array")
}

fn tokenize_with_pieces(server: &str, bytes: &[u8]) -> Result<Vec<NativeTokenPieceV0>> {
    let response = tokenize_response(server, bytes, true)?;
    tokens(&response)?
        .iter()
        .map(|value| {
            Ok(NativeTokenPieceV0 {
                id: token_id(value)?,
                bytes: piece_bytes(value)?,
            })
        })
        .collect()
}

fn full_tokenize(server: &str, bytes: &[u8]) -> Result<Vec<u32>> {
    let response = tokenize_response(server, bytes, false)?;
    tokens(&response)?.iter().map(token_id).collect()
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("write JSON {}", path.display()))
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn checkpoint_from_p0(args: CheckpointFromP0Args) -> Result<()> {
    let bytes = fs::read(&args.p0_snapshot)
        .with_context(|| format!("read P0 snapshot {}", args.p0_snapshot.display()))?;
    let snapshot = ProteusSnapshotV0::from_json_slice(&bytes)?;
    let checkpoint = create_checkpoint(&args.checkpoint, &snapshot)?;
    print_json(&json!({
        "schema": "proteus-p1-checkpoint-from-p0-v1",
        "checkpoint": args.checkpoint,
        "checkpoint_sequence": checkpoint.sequence,
        "checkpoint_root": hash_hex(&checkpoint.root),
        "transcript_len": checkpoint.snapshot.transcript_len,
        "transcript_sha256": checkpoint.snapshot.transcript_sha256,
    }))
}

fn append(args: AppendArgs) -> Result<()> {
    if args.receipt == args.checkpoint || args.receipt == args.journal {
        bail!("--receipt must be distinct from PRT0 and PJR0 paths");
    }
    let mut session = P1Session::open(&args.checkpoint, &args.journal)?;
    let expected_head = parse_hash_hex(&args.expected_head)?;
    let delta =
        fs::read(&args.delta).with_context(|| format!("read delta {}", args.delta.display()))?;
    let (delta_du, _): (LoadedStream, _) = load_du_stream(&args.delta_du_run, usize::MAX)
        .with_context(|| format!("load delta DU run {}", args.delta_du_run.display()))?;
    let next_sequence = session
        .snapshot()
        .append_sequence
        .checked_add(1)
        .context("append sequence overflow")?;
    let server = args.native_tokenizer_server.clone();
    let mut repair = |bytes: &[u8]| tokenize_with_pieces(&server, bytes);
    let server = args.native_tokenizer_server.clone();
    let mut oracle = |bytes: &[u8]| full_tokenize(&server, bytes);
    let receipt = session.append_verified(
        expected_head,
        &delta,
        &delta_du,
        args.drop_tokens,
        SnapshotProvenanceEventV0::now(next_sequence, args.actor, args.source),
        &mut repair,
        &mut oracle,
    )?;
    write_json(&args.receipt, &receipt)?;
    print_json(&receipt)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn paths_refer_to_same_file(left: &Path, right: &Path) -> Result<bool> {
    if !left.exists() || !right.exists() {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left = fs::metadata(left)?;
        let right = fs::metadata(right)?;
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(not(unix))]
    {
        Ok(fs::canonicalize(left)? == fs::canonicalize(right)?)
    }
}

fn resolved_destination(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().filter(|path| !path.as_os_str().is_empty());
    let file_name = path.file_name().context("output path must name a file")?;
    let current_dir = std::env::current_dir().context("resolve current directory")?;
    let absolute_parent = match parent {
        Some(parent) if parent.is_absolute() => parent.to_owned(),
        Some(parent) => current_dir.join(parent),
        None => current_dir,
    };
    let resolved_parent = fs::canonicalize(&absolute_parent).unwrap_or(absolute_parent);
    Ok(resolved_parent.join(file_name))
}

fn warm_bench(args: WarmBenchArgs) -> Result<()> {
    if args.iterations == 0 {
        bail!("--iterations must be greater than zero");
    }
    if args.receipt == args.checkpoint || args.receipt == args.journal {
        bail!("--receipt must be distinct from PRT0 and PJR0 paths");
    }
    if matches!(
        fs::symlink_metadata(&args.receipt),
        Ok(metadata) if metadata.file_type().is_symlink()
    ) {
        bail!("--receipt must not be a symbolic link");
    }
    if resolved_destination(&args.receipt)? == resolved_destination(&args.checkpoint)?
        || resolved_destination(&args.receipt)? == resolved_destination(&args.journal)?
    {
        bail!("--receipt must resolve to a distinct PRT0/PJR0 destination");
    }
    if paths_refer_to_same_file(&args.receipt, &args.checkpoint)?
        || paths_refer_to_same_file(&args.receipt, &args.journal)?
    {
        bail!("--receipt must not alias the PRT0 or PJR0 file");
    }

    let expected_head = parse_hash_hex(&args.expected_head)?;
    let initial_load_started = Instant::now();
    let loaded = P1Session::open(&args.checkpoint, &args.journal)?;
    let initial_head = loaded.head();
    let initial_transcript_len = loaded.snapshot().transcript_len;
    let mut session = P1WarmSession::from_loaded(loaded, expected_head)?;
    let initial_load_wall_ns = initial_load_started.elapsed().as_nanos();

    // All remaining input and mutation checks are outside the measured loop.
    let delta =
        fs::read(&args.delta).with_context(|| format!("read delta {}", args.delta.display()))?;
    let (delta_du, _): (LoadedStream, _) = load_du_stream(&args.delta_du_run, usize::MAX)
        .with_context(|| format!("load delta DU run {}", args.delta_du_run.display()))?;
    let journal_before = read_optional(&args.journal)?;

    let mut samples = Vec::with_capacity(args.iterations);
    let server = args.native_tokenizer_server.clone();
    let mut repair = |bytes: &[u8]| tokenize_with_pieces(&server, bytes);
    let server = args.native_tokenizer_server.clone();
    let mut oracle = |bytes: &[u8]| full_tokenize(&server, bytes);
    for iteration in 1..=args.iterations {
        let next_sequence = session
            .snapshot()
            .append_sequence
            .checked_add(1)
            .context("warm append sequence overflow")?;
        samples.push(session.append_verified(
            iteration,
            &delta,
            &delta_du,
            args.drop_tokens,
            SnapshotProvenanceEventV0::now(next_sequence, args.actor.clone(), args.source.clone()),
            &mut repair,
            &mut oracle,
        )?);
    }

    let journal_after = read_optional(&args.journal)?;
    if journal_after != journal_before {
        bail!("PJR0 journal changed during warm benchmark");
    }
    let receipt = P1WarmBenchReceipt {
        schema: "proteus-p1-warm-bench-v1",
        iterations: args.iterations,
        initial_head: hash_hex(&initial_head),
        final_head: hash_hex(&session.head()),
        initial_transcript_len,
        final_transcript_len: session.snapshot().transcript_len,
        initial_load_wall_ns,
        all_ids_exact: true,
        oracle_used_only_for_verification: true,
        file_io_inside_warm_loop: false,
        journal_mutated: false,
        samples,
    };
    write_json(&args.receipt, &receipt)?;
    if read_optional(&args.journal)? != journal_before {
        bail!("PJR0 journal changed while emitting warm benchmark receipt");
    }
    print_json(&receipt)
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::CheckpointFromP0(args) => checkpoint_from_p0(args),
        Command::Inspect(args) => print_json(&inspect(&args.checkpoint, &args.journal)?),
        Command::Append(args) => append(args),
        Command::WarmBench(args) => warm_bench(args),
        Command::FullAudit(args) => print_json(&full_audit(&args.checkpoint, &args.journal)?),
        Command::Fold(args) => {
            let folded = fold_checkpoint(&args.checkpoint, &args.journal, &args.output_checkpoint)?;
            print_json(&json!({
                "schema": "proteus-p1-fold-v1",
                "output_checkpoint": args.output_checkpoint,
                "checkpoint_sequence": folded.sequence,
                "checkpoint_root": hash_hex(&folded.root),
                "transcript_len": folded.snapshot.transcript_len,
                "transcript_sha256": folded.snapshot.transcript_sha256,
                "dictionary_next_id": folded.snapshot.dictionary_layers.next_id,
            }))
        }
    }
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("FAIL: {error:#}");
        std::process::exit(1);
    }
}
