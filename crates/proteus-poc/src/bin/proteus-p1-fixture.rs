use anyhow::{bail, Context, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};
use nereids::{import_native_token_jsonl, load_du_stream, sha256_hex, LoadedStream};
use proteus_poc::p1::{
    create_checkpoint, create_checkpoint_from_exact_bytes, fold_checkpoint, full_audit, hash_hex,
    inspect, parse_hash_hex, P1ExactIngestV1, P1Session, P1WarmBenchReceipt, P1WarmBenchSample,
    P1WarmSession,
};
use proteus_poc::{
    ModelTokenizerIdentityV0, NativeTokenPieceV0, ProteusSnapshotV0, SnapshotProvenanceEventV0,
};
use serde::{Deserialize, Serialize};
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
    /// Build an immutable checkpoint from exact raw/native/DU artifacts.
    CheckpointFromBytes(CheckpointFromBytesArgs),
    /// One-time initialization bridge from a fully validated P0 JSON snapshot.
    CheckpointFromP0(CheckpointFromP0Args),
    /// Inspect the accepted checkpoint/journal state as JSON.
    Inspect(StoreArgs),
    /// Construct, oracle-check, and CAS-commit one exact append.
    Append(AppendArgs),
    /// Measure sequential same-process in-memory appends without persistence.
    WarmBench(WarmBenchArgs),
    /// Replay manifest turns sequentially with configurable oracle checks.
    WarmReplay(WarmReplayArgs),
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
struct CheckpointFromBytesArgs {
    /// Exact transcript bytes; no text decoding or normalization is applied.
    #[arg(long)]
    transcript_bytes: PathBuf,
    /// Exact DU run artifact that reconstructs --transcript-bytes.
    #[arg(long)]
    du_run: PathBuf,
    /// Exact native token JSONL with IDs, byte spans, and bytes_hex.
    #[arg(long)]
    native_token_jsonl: PathBuf,
    #[arg(long)]
    model_id: String,
    #[arg(long)]
    model_provenance: String,
    #[arg(long)]
    tokenizer_id: String,
    #[arg(long)]
    tokenizer_provenance: String,
    #[arg(long)]
    dictionary_lineage: String,
    #[arg(long)]
    actor: String,
    #[arg(long)]
    source: String,
    /// New immutable PRT0 path; an existing file is never overwritten.
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
struct WarmReplayArgs {
    #[arg(long)]
    checkpoint: PathBuf,
    #[arg(long)]
    journal: PathBuf,
    #[arg(long)]
    expected_head: String,
    /// JSON manifest; relative turn paths resolve beside this file.
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    drop_tokens: usize,
    #[arg(long)]
    native_tokenizer_server: String,
    /// Verify every Nth turn (1 = every turn, 0 = no periodic checks).
    #[arg(long, default_value_t = 1)]
    oracle_every: usize,
    /// Disable the automatic final-turn oracle check.
    #[arg(long)]
    no_final_oracle: bool,
    #[arg(long)]
    actor: String,
    #[arg(long)]
    receipt: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WarmReplayManifest {
    schema: String,
    turns: Vec<WarmReplayManifestTurn>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WarmReplayManifestTurn {
    delta_file: PathBuf,
    delta_du_run: PathBuf,
    source: String,
    #[serde(default)]
    force_oracle: bool,
}

struct LoadedReplayTurn {
    delta_file: PathBuf,
    delta_du_run: PathBuf,
    source: String,
    force_oracle: bool,
    delta: Vec<u8>,
    delta_du: LoadedStream,
}

#[derive(Debug, Serialize)]
struct WarmReplayTurnReceipt {
    turn: usize,
    source: String,
    delta_file: PathBuf,
    delta_du_run: PathBuf,
    delta_len: usize,
    delta_sha256: String,
    force_oracle: bool,
    oracle_checked: bool,
    native_ids_exact: Option<bool>,
    candidate_native_ids_sha256_u32le: String,
    head: String,
    dictionary_lineage: String,
    dictionary_next_id: u64,
    native_token_count: usize,
    native_span_count: usize,
    du_token_count: usize,
    native_spans_exact: bool,
    du_lineage_validated: bool,
    candidate_constructed_before_oracle: bool,
    #[serde(flatten)]
    sample: P1WarmBenchSample,
}

#[derive(Debug, Serialize)]
struct WarmReplayFirstMismatch {
    turn: usize,
    candidate_native_ids_sha256_u32le: String,
    oracle_native_ids_sha256_u32le: String,
}

#[derive(Debug, Serialize)]
struct WarmReplayReceipt {
    schema: &'static str,
    #[serde(rename = "ALL_TURNS_EXACT", skip_serializing_if = "Option::is_none")]
    all_turns_exact: Option<bool>,
    total_turns: usize,
    oracle_calls: usize,
    oracle_verified_turn_count: usize,
    oracle_verified_turns: Vec<usize>,
    oracle_every: usize,
    final_oracle_requested: bool,
    final_oracle_verified: bool,
    forced_oracle_turns: Vec<usize>,
    first_mismatch: Option<WarmReplayFirstMismatch>,
    initial_head: String,
    final_head: String,
    initial_transcript_len: usize,
    final_transcript_len: usize,
    initial_load_wall_ns: u128,
    candidate_constructed_before_oracle: bool,
    file_io_inside_warm_compute: bool,
    journal_mutated: bool,
    turns: Vec<WarmReplayTurnReceipt>,
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

fn checkpoint_from_bytes(args: CheckpointFromBytesArgs) -> Result<()> {
    let transcript = fs::read(&args.transcript_bytes).with_context(|| {
        format!(
            "read exact transcript bytes {}",
            args.transcript_bytes.display()
        )
    })?;
    let (du_stream, _) = load_du_stream(&args.du_run, usize::MAX)
        .with_context(|| format!("load exact DU run {}", args.du_run.display()))?;
    let native_tokenization = import_native_token_jsonl(
        &args.native_token_jsonl,
        &args.tokenizer_id,
        &args.tokenizer_provenance,
        &transcript,
    )
    .with_context(|| {
        format!(
            "load exact native token spans {}",
            args.native_token_jsonl.display()
        )
    })?;
    let checkpoint = create_checkpoint_from_exact_bytes(
        &args.checkpoint,
        &transcript,
        P1ExactIngestV1 {
            native_tokenization,
            du_stream,
            dictionary_lineage: args.dictionary_lineage,
            model_tokenizer: ModelTokenizerIdentityV0 {
                model_id: args.model_id,
                model_provenance: args.model_provenance,
                tokenizer_id: args.tokenizer_id,
                tokenizer_provenance: args.tokenizer_provenance,
            },
            creation_provenance: SnapshotProvenanceEventV0::now(0, args.actor, args.source),
        },
    )?;
    print_json(&json!({
        "schema": "proteus-p1-checkpoint-from-exact-bytes-v1",
        "checkpoint": args.checkpoint,
        "checkpoint_sequence": checkpoint.sequence,
        "checkpoint_root": hash_hex(&checkpoint.root),
        "transcript_len": checkpoint.snapshot.transcript_len,
        "transcript_sha256": checkpoint.snapshot.transcript_sha256,
        "native_token_count": checkpoint.snapshot.direct_id.native_ids.len(),
        "du_token_count": checkpoint.snapshot.direct_id.du_state.ids.len(),
        "dictionary_next_id": checkpoint.snapshot.dictionary_layers.next_id,
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

fn validate_receipt_destination(receipt: &Path, checkpoint: &Path, journal: &Path) -> Result<()> {
    if receipt == checkpoint || receipt == journal {
        bail!("--receipt must be distinct from PRT0 and PJR0 paths");
    }
    if matches!(
        fs::symlink_metadata(receipt),
        Ok(metadata) if metadata.file_type().is_symlink()
    ) {
        bail!("--receipt must not be a symbolic link");
    }
    if resolved_destination(receipt)? == resolved_destination(checkpoint)?
        || resolved_destination(receipt)? == resolved_destination(journal)?
    {
        bail!("--receipt must resolve to a distinct PRT0/PJR0 destination");
    }
    if paths_refer_to_same_file(receipt, checkpoint)? || paths_refer_to_same_file(receipt, journal)?
    {
        bail!("--receipt must not alias the PRT0 or PJR0 file");
    }
    Ok(())
}

fn warm_bench(args: WarmBenchArgs) -> Result<()> {
    if args.iterations == 0 {
        bail!("--iterations must be greater than zero");
    }
    validate_receipt_destination(&args.receipt, &args.checkpoint, &args.journal)?;

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

fn manifest_relative_path(manifest: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        manifest
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}

fn sha256_ids_u32le(ids: &[u32]) -> String {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(ids));
    for id in ids {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    sha256_hex(&bytes)
}

fn oracle_requested_for_turn(
    turn: usize,
    total_turns: usize,
    oracle_every: usize,
    final_oracle_requested: bool,
    force_oracle: bool,
) -> bool {
    force_oracle
        || (oracle_every != 0 && turn % oracle_every == 0)
        || (final_oracle_requested && turn == total_turns)
}

fn all_turns_exact(
    total_turns: usize,
    oracle_verified_turns: &[usize],
    first_mismatch: &Option<WarmReplayFirstMismatch>,
) -> Option<bool> {
    (first_mismatch.is_none() && oracle_verified_turns.len() == total_turns).then_some(true)
}

fn warm_replay(args: WarmReplayArgs) -> Result<()> {
    validate_receipt_destination(&args.receipt, &args.checkpoint, &args.journal)?;
    if resolved_destination(&args.receipt)? == resolved_destination(&args.manifest)?
        || paths_refer_to_same_file(&args.receipt, &args.manifest)?
    {
        bail!("--receipt must not overwrite or alias --manifest");
    }

    let expected_head = parse_hash_hex(&args.expected_head)?;
    let initial_load_started = Instant::now();
    let loaded = P1Session::open(&args.checkpoint, &args.journal)?;
    let initial_head = loaded.head();
    let initial_transcript_len = loaded.snapshot().transcript_len;
    let mut session = P1WarmSession::from_loaded(loaded, expected_head)?;
    let initial_load_wall_ns = initial_load_started.elapsed().as_nanos();

    // Read and validate every workload artifact before measured warm work.
    let manifest_bytes = fs::read(&args.manifest)
        .with_context(|| format!("read replay manifest {}", args.manifest.display()))?;
    let manifest: WarmReplayManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("parse replay manifest {}", args.manifest.display()))?;
    if manifest.schema != "proteus-p1-diverse-replay-manifest-v1" {
        bail!("unsupported replay manifest schema {:?}", manifest.schema);
    }
    if manifest.turns.is_empty() {
        bail!("replay manifest must contain at least one turn");
    }

    let mut loaded_turns = Vec::<LoadedReplayTurn>::with_capacity(manifest.turns.len());
    for (index, turn) in manifest.turns.into_iter().enumerate() {
        if turn.source.trim().is_empty() {
            bail!("replay manifest turn {} has an empty source", index + 1);
        }
        let delta_file = manifest_relative_path(&args.manifest, &turn.delta_file);
        let delta_du_run = manifest_relative_path(&args.manifest, &turn.delta_du_run);
        if resolved_destination(&args.receipt)? == resolved_destination(&delta_file)?
            || paths_refer_to_same_file(&args.receipt, &delta_file)?
        {
            bail!("--receipt must not overwrite or alias a delta byte file");
        }
        let delta = fs::read(&delta_file)
            .with_context(|| format!("read replay delta {}", delta_file.display()))?;
        if delta.is_empty() {
            bail!("replay manifest turn {} has an empty delta", index + 1);
        }
        let (delta_du, _): (LoadedStream, _) = load_du_stream(&delta_du_run, usize::MAX)
            .with_context(|| format!("load replay DU run {}", delta_du_run.display()))?;
        if delta_du.reconstruct()? != delta {
            bail!(
                "replay manifest turn {} DU artifact does not reconstruct its exact delta bytes",
                index + 1
            );
        }
        loaded_turns.push(LoadedReplayTurn {
            delta_file,
            delta_du_run,
            source: turn.source,
            force_oracle: turn.force_oracle,
            delta,
            delta_du,
        });
    }
    let journal_before = read_optional(&args.journal)?;

    let server = args.native_tokenizer_server.clone();
    let mut repair = |bytes: &[u8]| tokenize_with_pieces(&server, bytes);
    let server = args.native_tokenizer_server.clone();
    let total_turns = loaded_turns.len();
    let final_oracle_requested = !args.no_final_oracle;
    let forced_oracle_turns = loaded_turns
        .iter()
        .enumerate()
        .filter_map(|(index, turn)| turn.force_oracle.then_some(index + 1))
        .collect::<Vec<_>>();
    let mut oracle_calls = 0usize;
    let mut oracle_verified_turns = Vec::new();
    let mut final_oracle_verified = false;
    let mut first_mismatch = None;
    let mut turns = Vec::with_capacity(loaded_turns.len());
    for (index, turn) in loaded_turns.iter().enumerate() {
        let turn_number = index + 1;
        let next_sequence = session
            .snapshot()
            .append_sequence
            .checked_add(1)
            .context("warm replay append sequence overflow")?;
        let mut sample = session.append_candidate(
            turn_number,
            &turn.delta,
            &turn.delta_du,
            args.drop_tokens,
            SnapshotProvenanceEventV0 {
                append_sequence: next_sequence,
                unix_time_ns: 0,
                actor: args.actor.clone(),
                source: turn.source.clone(),
            },
            &mut repair,
        )?;
        let candidate_native_ids_sha256_u32le =
            sha256_ids_u32le(&session.snapshot().direct_id.native_ids);
        let head = hash_hex(&session.head());
        let should_call_oracle = oracle_requested_for_turn(
            turn_number,
            total_turns,
            args.oracle_every,
            final_oracle_requested,
            turn.force_oracle,
        );
        let native_ids_exact = if should_call_oracle {
            // append_candidate has already completed the candidate, evolved
            // snapshot, transcript identity, and logical head.
            let transcript = session.snapshot().direct_id.du_state.reconstruct()?;
            oracle_calls += 1;
            let oracle_started = Instant::now();
            let oracle_ids = full_tokenize(&server, &transcript)?;
            sample.oracle_full_tokenize_wall_ns = oracle_started.elapsed().as_nanos();
            let ids_exact = session.snapshot().direct_id.native_ids == oracle_ids;
            if ids_exact {
                oracle_verified_turns.push(turn_number);
            } else if first_mismatch.is_none() {
                first_mismatch = Some(WarmReplayFirstMismatch {
                    turn: turn_number,
                    candidate_native_ids_sha256_u32le: candidate_native_ids_sha256_u32le.clone(),
                    oracle_native_ids_sha256_u32le: sha256_ids_u32le(&oracle_ids),
                });
            }
            if turn_number == total_turns {
                final_oracle_verified = ids_exact;
            }
            Some(ids_exact)
        } else {
            None
        };
        let snapshot = session.snapshot();
        turns.push(WarmReplayTurnReceipt {
            turn: turn_number,
            source: turn.source.clone(),
            delta_file: turn.delta_file.clone(),
            delta_du_run: turn.delta_du_run.clone(),
            delta_len: turn.delta.len(),
            delta_sha256: sha256_hex(&turn.delta),
            force_oracle: turn.force_oracle,
            oracle_checked: should_call_oracle,
            native_ids_exact,
            candidate_native_ids_sha256_u32le,
            head,
            dictionary_lineage: snapshot.dictionary_lineage.clone(),
            dictionary_next_id: snapshot.dictionary_layers.next_id,
            native_token_count: snapshot.direct_id.native_ids.len(),
            native_span_count: snapshot.direct_id.native_spans.len(),
            du_token_count: snapshot.direct_id.du_state.ids.len(),
            native_spans_exact: true,
            du_lineage_validated: true,
            candidate_constructed_before_oracle: true,
            sample,
        });
    }
    if read_optional(&args.journal)? != journal_before {
        bail!("PJR0 journal changed during warm replay");
    }

    let receipt = WarmReplayReceipt {
        schema: "proteus-p1-diverse-warm-replay-v1",
        all_turns_exact: all_turns_exact(total_turns, &oracle_verified_turns, &first_mismatch),
        total_turns,
        oracle_calls,
        oracle_verified_turn_count: oracle_verified_turns.len(),
        oracle_verified_turns,
        oracle_every: args.oracle_every,
        final_oracle_requested,
        final_oracle_verified,
        forced_oracle_turns,
        first_mismatch,
        initial_head: hash_hex(&initial_head),
        final_head: hash_hex(&session.head()),
        initial_transcript_len,
        final_transcript_len: session.snapshot().transcript_len,
        initial_load_wall_ns,
        candidate_constructed_before_oracle: true,
        file_io_inside_warm_compute: false,
        journal_mutated: false,
        turns,
    };
    write_json(&args.receipt, &receipt)?;
    if read_optional(&args.journal)? != journal_before {
        bail!("PJR0 journal changed while emitting warm replay receipt");
    }
    print_json(&receipt)
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::CheckpointFromBytes(args) => checkpoint_from_bytes(args),
        Command::CheckpointFromP0(args) => checkpoint_from_p0(args),
        Command::Inspect(args) => print_json(&inspect(&args.checkpoint, &args.journal)?),
        Command::Append(args) => append(args),
        Command::WarmBench(args) => warm_bench(args),
        Command::WarmReplay(args) => warm_replay(args),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn requested_turns(
        total_turns: usize,
        oracle_every: usize,
        final_oracle_requested: bool,
        forced_turn: Option<usize>,
    ) -> Vec<usize> {
        (1..=total_turns)
            .filter(|turn| {
                oracle_requested_for_turn(
                    *turn,
                    total_turns,
                    oracle_every,
                    final_oracle_requested,
                    forced_turn == Some(*turn),
                )
            })
            .collect()
    }

    fn receipt_with_verification(
        total_turns: usize,
        oracle_verified_turns: Vec<usize>,
    ) -> WarmReplayReceipt {
        WarmReplayReceipt {
            schema: "proteus-p1-diverse-warm-replay-v1",
            all_turns_exact: all_turns_exact(total_turns, &oracle_verified_turns, &None),
            total_turns,
            oracle_calls: oracle_verified_turns.len(),
            oracle_verified_turn_count: oracle_verified_turns.len(),
            oracle_verified_turns,
            oracle_every: 2,
            final_oracle_requested: true,
            final_oracle_verified: true,
            forced_oracle_turns: Vec::new(),
            first_mismatch: None,
            initial_head: "00".repeat(32),
            final_head: "11".repeat(32),
            initial_transcript_len: 0,
            final_transcript_len: 0,
            initial_load_wall_ns: 0,
            candidate_constructed_before_oracle: true,
            file_io_inside_warm_compute: false,
            journal_mutated: false,
            turns: Vec::new(),
        }
    }

    #[test]
    fn oracle_every_one_verifies_every_turn() {
        assert_eq!(requested_turns(5, 1, true, None), [1, 2, 3, 4, 5]);
        assert_eq!(all_turns_exact(5, &[1, 2, 3, 4, 5], &None), Some(true));
    }

    #[test]
    fn oracle_every_n_verifies_scheduled_turns() {
        assert_eq!(requested_turns(7, 3, false, None), [3, 6]);
    }

    #[test]
    fn forced_turn_is_verified_without_periodic_or_final_policy() {
        assert_eq!(requested_turns(5, 0, false, Some(2)), [2]);
    }

    #[test]
    fn final_turn_is_verified_by_default() {
        assert_eq!(requested_turns(5, 0, true, None), [5]);
    }

    #[test]
    fn zero_periodic_and_disabled_final_makes_no_unforced_calls() {
        assert!(requested_turns(5, 0, false, None).is_empty());
    }

    #[test]
    fn sampled_receipt_does_not_emit_all_turns_exact() {
        let receipt = receipt_with_verification(5, vec![2, 4, 5]);
        let json = serde_json::to_value(receipt).unwrap();
        assert!(json.get("ALL_TURNS_EXACT").is_none());
        assert_eq!(json["oracle_verified_turn_count"], 3);
        assert_eq!(json["oracle_verified_turns"], serde_json::json!([2, 4, 5]));
    }
}
