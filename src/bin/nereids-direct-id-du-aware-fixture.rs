use anyhow::{bail, Context, Result};
use clap::Parser;
use nereids::direct_id::{
    construct_du_aware_repair_candidate, search_exact_du_aware_repair, DirectIdSnapshotV2,
    DuAwareExactRepair, DuAwareRepairTrial,
};
use nereids::{import_native_token_jsonl, load_du_stream, sha256_hex};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Parser)]
#[command(name = "nereids-direct-id-du-aware-fixture")]
#[command(about = "Oracle-verified Direct-ID V2 repair from persisted spans and DU state")]
struct Args {
    /// Raw prefix bytes used to verify the persisted snapshot inputs.
    #[arg(long)]
    prefix: PathBuf,

    /// Raw delta bytes used only for DU identity and final decode checks.
    #[arg(long)]
    delta: PathBuf,

    /// DU run whose complete structural stream reconstructs the prefix.
    #[arg(long)]
    prefix_du_run: PathBuf,

    /// DU run whose complete structural stream reconstructs the delta.
    #[arg(long)]
    delta_du_run: PathBuf,

    /// Persisted native token records with ID, byte span, and exact bytes.
    #[arg(long)]
    native_spans: PathBuf,

    #[arg(long, default_value = "external:persisted-native-spans")]
    native_spans_provenance: String,

    #[arg(long)]
    native_tokenizer_server: String,

    #[arg(long)]
    model_name: String,

    /// Inclusive diagnostic search bound when --drop-tokens is omitted.
    #[arg(long, default_value_t = 32)]
    max_drop_tokens: usize,

    /// Construct exactly this drop depth without using the oracle to select it.
    #[arg(long)]
    drop_tokens: Option<usize>,

    #[arg(long)]
    receipt: PathBuf,
}

#[derive(Debug)]
struct TimedTokens {
    ids: Vec<u32>,
    wall_ns: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepairSelectionMode {
    FixedDrop,
    OracleSearch,
}

impl RepairSelectionMode {
    fn receipt_name(self) -> &'static str {
        match self {
            Self::FixedDrop => "fixed_drop",
            Self::OracleSearch => "oracle_search",
        }
    }

    fn oracle_used_for_drop_selection(self) -> bool {
        matches!(self, Self::OracleSearch)
    }

    fn oracle_used_for_candidate_construction(self) -> bool {
        false
    }

    fn oracle_used_for_verification(self) -> bool {
        true
    }
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
    du_delta_reconstruction_exact: bool,
    native_snapshot_token_count: usize,
    native_snapshot_span_count: usize,
    native_snapshot_spans_exact: bool,
    drop_tokens: usize,
    repair_start_byte: usize,
    snapshot_tail_bytes: usize,
    repair_window_bytes: usize,
    repair_token_count: usize,
    candidate_token_count: usize,
    oracle_token_count: usize,
    candidate_ids_sha256_u32le: String,
    oracle_ids_sha256_u32le: String,
    ids_exact: bool,
    decode_exact: bool,
    candidate_used_full_window: bool,
    candidate_used_full_prefix_detokenize: bool,
    repair_selection_mode: &'static str,
    oracle_used_for_candidate_construction: bool,
    oracle_used_for_drop_selection: bool,
    oracle_used_for_verification: bool,
    du_reconstruct_wall_ns: u128,
    span_lookup_wall_ns: u128,
    repair_tokenize_wall_ns: u128,
    splice_wall_ns: u128,
    candidate_total_prep_wall_ns: u128,
    oracle_full_tokenize_wall_ns: u128,
    trials: Vec<DuAwareRepairTrial>,
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
    let started = Instant::now();
    let content = std::str::from_utf8(bytes).context("tokenizer input is not valid UTF-8")?;
    let body = serde_json::to_string(&json!({
        "content": content,
        "add_special": false
    }))?;
    let response = http_post_json(server, "/tokenize", &body)?;
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
    let wall_ns = started.elapsed().as_nanos();
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

fn make_receipt(
    prefix: &[u8],
    delta: &[u8],
    snapshot: &DirectIdSnapshotV2,
    oracle: &TimedTokens,
    repair: DuAwareExactRepair,
    decoded: &[u8],
    model_name: String,
    selection_mode: RepairSelectionMode,
) -> Result<Receipt> {
    let mut full = Vec::with_capacity(prefix.len() + delta.len());
    full.extend_from_slice(prefix);
    full.extend_from_slice(delta);

    let candidate = repair.candidate;
    let du_delta_reconstruction_exact = repair.reconstructed_delta.as_slice() == delta;
    let ids_exact = candidate.ids == oracle.ids;
    let decode_exact = decoded == full.as_slice();
    let candidate_used_full_window = candidate.repair_start_byte == 0;
    let candidate_used_full_prefix_detokenize = false;

    if !du_delta_reconstruction_exact {
        bail!("DU delta reconstruction differs from the supplied delta bytes");
    }
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
        schema: "nereids-direct-id-du-aware-v2",
        model_name,
        prefix_bytes: prefix.len(),
        delta_bytes: delta.len(),
        full_bytes: full.len(),
        prefix_sha256: sha256_hex(prefix),
        delta_sha256: sha256_hex(delta),
        full_sha256: sha256_hex(&full),
        du_delta_reconstruction_exact,
        native_snapshot_token_count: snapshot.native_ids.len(),
        native_snapshot_span_count: snapshot.native_spans.len(),
        native_snapshot_spans_exact: true,
        drop_tokens: candidate.drop_tokens,
        repair_start_byte: candidate.repair_start_byte,
        snapshot_tail_bytes: candidate.snapshot_tail_bytes,
        repair_window_bytes: candidate.repair_window_bytes,
        repair_token_count: candidate.repair_token_count,
        candidate_token_count: candidate.ids.len(),
        oracle_token_count: oracle.ids.len(),
        candidate_ids_sha256_u32le: sha256_ids_u32le(&candidate.ids),
        oracle_ids_sha256_u32le: sha256_ids_u32le(&oracle.ids),
        ids_exact,
        decode_exact,
        candidate_used_full_window,
        candidate_used_full_prefix_detokenize,
        repair_selection_mode: selection_mode.receipt_name(),
        oracle_used_for_candidate_construction: selection_mode
            .oracle_used_for_candidate_construction(),
        oracle_used_for_drop_selection: selection_mode.oracle_used_for_drop_selection(),
        oracle_used_for_verification: selection_mode.oracle_used_for_verification(),
        du_reconstruct_wall_ns: repair.timings.du_reconstruct_wall_ns,
        span_lookup_wall_ns: repair.timings.span_lookup_wall_ns,
        repair_tokenize_wall_ns: repair.timings.repair_tokenize_wall_ns,
        splice_wall_ns: repair.timings.splice_wall_ns,
        candidate_total_prep_wall_ns: repair.timings.candidate_total_prep_wall_ns,
        oracle_full_tokenize_wall_ns: oracle.wall_ns,
        trials: repair.trials,
    })
}

fn run(args: Args) -> Result<()> {
    let prefix =
        fs::read(&args.prefix).with_context(|| format!("read prefix {}", args.prefix.display()))?;
    let delta =
        fs::read(&args.delta).with_context(|| format!("read delta {}", args.delta.display()))?;
    if prefix.is_empty() {
        bail!("prefix must be nonempty so candidate repair cannot become a full-window fallback");
    }

    let (prefix_du, _) = load_du_stream(&args.prefix_du_run, usize::MAX)
        .with_context(|| format!("load prefix DU run {}", args.prefix_du_run.display()))?;
    let (delta_du, _) = load_du_stream(&args.delta_du_run, usize::MAX)
        .with_context(|| format!("load delta DU run {}", args.delta_du_run.display()))?;
    let native_stream = import_native_token_jsonl(
        &args.native_spans,
        &args.model_name,
        &args.native_spans_provenance,
        &prefix,
    )
    .with_context(|| format!("load native snapshot spans {}", args.native_spans.display()))?;
    let snapshot = DirectIdSnapshotV2::from_tokenizer_stream(&prefix, &native_stream, prefix_du)
        .context("validate persisted Direct-ID V2 snapshot")?;

    let reconstructed_delta = delta_du
        .reconstruct()
        .context("preflight DU delta reconstruction")?;
    if reconstructed_delta != delta {
        bail!("delta DU stream does not reconstruct the supplied delta bytes exactly");
    }

    let mut tokenize_repair_window = |bytes: &[u8]| {
        tokenize(&args.native_tokenizer_server, bytes)
            .map(|timed| timed.ids)
            .map_err(|error| error.to_string())
    };
    let (repair, oracle, selection_mode) = if let Some(drop_tokens) = args.drop_tokens {
        // Fixed-drop acceptance is oracle-independent through the complete
        // candidate construction and timing boundary.
        let (candidate, timings) = construct_du_aware_repair_candidate(
            &snapshot,
            &delta_du,
            drop_tokens,
            &mut tokenize_repair_window,
        )
        .map_err(anyhow::Error::new)?;

        // Construct and time the full-tokenization oracle only after candidate
        // timing has finished. Equality and full decode verification follow it.
        let mut full = Vec::with_capacity(prefix.len() + delta.len());
        full.extend_from_slice(&prefix);
        full.extend_from_slice(&delta);
        let oracle = tokenize(&args.native_tokenizer_server, &full)
            .context("native-tokenize full verification oracle")?;
        let ids_exact = candidate.ids == oracle.ids;
        let trials = vec![DuAwareRepairTrial {
            drop_tokens: candidate.drop_tokens,
            kept_snapshot_token_count: Some(candidate.kept_snapshot_token_count),
            repair_start_byte: Some(candidate.repair_start_byte),
            snapshot_tail_bytes: Some(candidate.snapshot_tail_bytes),
            repair_window_bytes: Some(candidate.repair_window_bytes),
            repair_token_count: Some(candidate.repair_token_count),
            candidate_token_count: Some(candidate.ids.len()),
            ids_exact,
            rejected: None,
        }];
        let repair = DuAwareExactRepair {
            candidate,
            reconstructed_delta,
            timings,
            trials,
        };
        (repair, oracle, RepairSelectionMode::FixedDrop)
    } else {
        // Diagnostic search uses the oracle to select the first exact drop
        // depth, while each individual candidate remains oracle-independent.
        let mut full = Vec::with_capacity(prefix.len() + delta.len());
        full.extend_from_slice(&prefix);
        full.extend_from_slice(&delta);
        let oracle = tokenize(&args.native_tokenizer_server, &full)
            .context("native-tokenize full diagnostic oracle")?;
        let repair = search_exact_du_aware_repair(
            &snapshot,
            &delta_du,
            &oracle.ids,
            args.max_drop_tokens,
            &mut tokenize_repair_window,
        )
        .map_err(anyhow::Error::new)?;
        (repair, oracle, RepairSelectionMode::OracleSearch)
    };

    // This full candidate decode is verification only; it is not used for
    // boundary discovery or candidate construction.
    let decoded = detokenize(&args.native_tokenizer_server, &repair.candidate.ids)
        .context("verify candidate decode")?;
    let receipt = make_receipt(
        &prefix,
        &delta,
        &snapshot,
        &oracle,
        repair,
        &decoded,
        args.model_name,
        selection_mode,
    )?;
    write_receipt(&args.receipt, &receipt)?;

    println!("schema={}", receipt.schema);
    println!("model_name={}", receipt.model_name);
    println!(
        "du_delta_reconstruction_exact={}",
        receipt.du_delta_reconstruction_exact
    );
    println!(
        "native_snapshot_spans_exact={}",
        receipt.native_snapshot_spans_exact
    );
    println!("drop_tokens={}", receipt.drop_tokens);
    println!("repair_start_byte={}", receipt.repair_start_byte);
    println!("repair_window_bytes={}", receipt.repair_window_bytes);
    println!("ids_exact={}", receipt.ids_exact);
    println!("decode_exact={}", receipt.decode_exact);
    println!(
        "candidate_used_full_window={}",
        receipt.candidate_used_full_window
    );
    println!(
        "candidate_used_full_prefix_detokenize={}",
        receipt.candidate_used_full_prefix_detokenize
    );
    println!("repair_selection_mode={}", receipt.repair_selection_mode);
    println!(
        "oracle_used_for_candidate_construction={}",
        receipt.oracle_used_for_candidate_construction
    );
    println!(
        "oracle_used_for_drop_selection={}",
        receipt.oracle_used_for_drop_selection
    );
    println!(
        "oracle_used_for_verification={}",
        receipt.oracle_used_for_verification
    );
    println!(
        "candidate_total_prep_wall_ns={}",
        receipt.candidate_total_prep_wall_ns
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

#[cfg(test)]
mod tests {
    use super::*;

    fn required_args() -> Vec<&'static str> {
        vec![
            "nereids-direct-id-du-aware-fixture",
            "--prefix",
            "prefix.txt",
            "--delta",
            "delta.txt",
            "--prefix-du-run",
            "prefix-du",
            "--delta-du-run",
            "delta-du",
            "--native-spans",
            "native.jsonl",
            "--native-tokenizer-server",
            "http://127.0.0.1:8080",
            "--model-name",
            "test-model",
            "--receipt",
            "receipt.json",
        ]
    }

    #[test]
    fn drop_tokens_is_optional_and_selects_fixed_drop() {
        let search_args = Args::try_parse_from(required_args()).unwrap();
        assert_eq!(search_args.drop_tokens, None);

        let mut fixed_args = required_args();
        fixed_args.extend(["--drop-tokens", "7"]);
        let fixed_args = Args::try_parse_from(fixed_args).unwrap();
        assert_eq!(fixed_args.drop_tokens, Some(7));
    }

    #[test]
    fn selection_modes_report_oracle_roles_exactly() {
        assert_eq!(RepairSelectionMode::FixedDrop.receipt_name(), "fixed_drop");
        assert!(!RepairSelectionMode::FixedDrop.oracle_used_for_candidate_construction());
        assert!(!RepairSelectionMode::FixedDrop.oracle_used_for_drop_selection());
        assert!(RepairSelectionMode::FixedDrop.oracle_used_for_verification());
        assert_eq!(
            RepairSelectionMode::OracleSearch.receipt_name(),
            "oracle_search"
        );
        assert!(!RepairSelectionMode::OracleSearch.oracle_used_for_candidate_construction());
        assert!(RepairSelectionMode::OracleSearch.oracle_used_for_drop_selection());
        assert!(RepairSelectionMode::OracleSearch.oracle_used_for_verification());
    }
}
