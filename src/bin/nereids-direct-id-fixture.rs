use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
struct Args {
    fixture: PathBuf,
    du_restored: PathBuf,
    native_tokenizer_server: String,
    model_name: String,
    receipt: PathBuf,
}

#[derive(Debug, Clone)]
struct TokenResult {
    ids: Vec<u32>,
    wall_ns: u128,
}

#[derive(Debug, Clone)]
struct DecodeResult {
    available: bool,
    exact: Option<bool>,
    wall_ns: Option<u128>,
    decoded_sha256: Option<String>,
    decoded_bytes: Option<usize>,
    error: Option<String>,
}

fn usage() -> &'static str {
    "usage: nereids-direct-id-fixture \\
  --fixture <path> \\
  --du-restored <path> \\
  --native-tokenizer-server <http://host:port> \\
  --model-name <name> \\
  --receipt <json>"
}

fn parse_args() -> Result<Args, String> {
    let mut fixture = None;
    let mut du_restored = None;
    let mut server = None;
    let mut model_name = None;
    let mut receipt = None;

    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--help" | "-h" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            "--fixture" => fixture = it.next().map(PathBuf::from),
            "--du-restored" => du_restored = it.next().map(PathBuf::from),
            "--native-tokenizer-server" => server = it.next(),
            "--model-name" => model_name = it.next(),
            "--receipt" => receipt = it.next().map(PathBuf::from),
            other => return Err(format!("unknown argument: {}", other)),
        }
    }

    Ok(Args {
        fixture: fixture.ok_or("--fixture missing")?,
        du_restored: du_restored.ok_or("--du-restored missing")?,
        native_tokenizer_server: server.ok_or("--native-tokenizer-server missing")?,
        model_name: model_name.ok_or("--model-name missing")?,
        receipt: receipt.ok_or("--receipt missing")?,
    })
}

fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn parse_http_base(base: &str) -> Result<(String, u16), String> {
    let s = base.trim().trim_end_matches('/');
    let s = s
        .strip_prefix("http://")
        .ok_or("only http:// server URLs are supported")?;

    let mut parts = s.split(':');
    let host = parts.next().ok_or("missing host")?.to_string();
    let port_s = parts.next().ok_or("missing port")?;
    let port = port_s
        .parse::<u16>()
        .map_err(|e| format!("bad port: {}", e))?;

    Ok((host, port))
}

fn http_post_json(base: &str, path: &str, body: &str) -> Result<String, String> {
    let (host, port) = parse_http_base(base)?;
    let mut stream =
        TcpStream::connect((host.as_str(), port)).map_err(|e| format!("connect failed: {}", e))?;

    let req = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        path,
        host,
        port,
        body.as_bytes().len(),
        body
    );

    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write failed: {}", e))?;

    let mut resp = String::new();
    stream
        .read_to_string(&mut resp)
        .map_err(|e| format!("read failed: {}", e))?;

    let mut split = resp.splitn(2, "\r\n\r\n");
    let head = split.next().unwrap_or("");
    let body = split.next().unwrap_or("").to_string();

    let status = head.lines().next().unwrap_or("");
    if !status.contains(" 200 ") && !status.contains(" 201 ") {
        return Err(format!("http error: {} body={}", status, body));
    }

    Ok(body)
}

fn parse_u32_array_after(json: &str, key: &str) -> Result<Vec<u32>, String> {
    let key_pos = json.find(key).ok_or(format!("missing key {}", key))?;
    let rest = &json[key_pos..];
    let start_rel = rest.find('[').ok_or("missing [")?;
    let start = key_pos + start_rel + 1;

    let mut end = None;
    for (i, ch) in json[start..].char_indices() {
        if ch == ']' {
            end = Some(start + i);
            break;
        }
    }

    let end = end.ok_or("missing ]")?;
    let inner = &json[start..end];

    let mut ids = Vec::new();
    for part in inner.split(',') {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        let v = t
            .parse::<u32>()
            .map_err(|e| format!("bad token id '{}': {}", t, e))?;
        ids.push(v);
    }

    Ok(ids)
}

fn parse_tokens(json: &str) -> Result<Vec<u32>, String> {
    if let Ok(v) = parse_u32_array_after(json, "\"tokens\"") {
        return Ok(v);
    }
    parse_u32_array_after(json, "\"ids\"")
}

fn parse_json_string_after(json: &str, key: &str) -> Result<String, String> {
    let key_pos = json.find(key).ok_or(format!("missing key {}", key))?;
    let rest = &json[key_pos..];
    let colon = rest.find(':').ok_or("missing colon")?;
    let after_colon = key_pos + colon + 1;

    let mut start = None;
    for (i, ch) in json[after_colon..].char_indices() {
        if ch == '"' {
            start = Some(after_colon + i + 1);
            break;
        }
    }

    let mut i = start.ok_or("missing string quote")?;
    let bytes = json.as_bytes();
    let mut out = String::new();

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            return Ok(out);
        }
        if b != b'\\' {
            let s =
                std::str::from_utf8(&bytes[i..]).map_err(|e| format!("utf8 parse error: {}", e))?;
            let ch = s.chars().next().ok_or("empty char")?;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }

        i += 1;
        if i >= bytes.len() {
            return Err("bad trailing escape".to_string());
        }

        match bytes[i] as char {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                if i + 4 >= bytes.len() {
                    return Err("short unicode escape".to_string());
                }
                let hex = &json[i + 1..i + 5];
                let cp = u32::from_str_radix(hex, 16)
                    .map_err(|e| format!("bad unicode escape: {}", e))?;
                if let Some(ch) = char::from_u32(cp) {
                    out.push(ch);
                } else {
                    return Err(format!("invalid unicode scalar: {}", cp));
                }
                i += 4;
            }
            other => return Err(format!("bad escape: {}", other)),
        }
        i += 1;
    }

    Err("unterminated JSON string".to_string())
}

fn tokenize(server: &str, text: &str) -> Result<TokenResult, String> {
    let body = format!(
        "{{\"content\":\"{}\",\"add_special\":false}}",
        json_escape(text)
    );

    let t0 = Instant::now();
    let resp = http_post_json(server, "/tokenize", &body)?;
    let wall_ns = t0.elapsed().as_nanos();

    let ids = parse_tokens(&resp)?;
    Ok(TokenResult { ids, wall_ns })
}

fn ids_json_array(ids: &[u32]) -> String {
    let mut s = String::from("[");
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&id.to_string());
    }
    s.push(']');
    s
}

fn detokenize(server: &str, ids: &[u32], expected: &[u8]) -> DecodeResult {
    let body = format!("{{\"tokens\":{},\"special\":false}}", ids_json_array(ids));

    let t0 = Instant::now();
    let resp = match http_post_json(server, "/detokenize", &body) {
        Ok(v) => v,
        Err(e) => {
            return DecodeResult {
                available: false,
                exact: None,
                wall_ns: None,
                decoded_sha256: None,
                decoded_bytes: None,
                error: Some(e),
            };
        }
    };
    let wall_ns = t0.elapsed().as_nanos();

    let content = parse_json_string_after(&resp, "\"content\"")
        .or_else(|_| parse_json_string_after(&resp, "\"text\""));

    match content {
        Ok(s) => {
            let b = s.as_bytes();
            DecodeResult {
                available: true,
                exact: Some(b == expected),
                wall_ns: Some(wall_ns),
                decoded_sha256: Some(sha256_bytes_via_tool(b).unwrap_or_else(|e| e)),
                decoded_bytes: Some(b.len()),
                error: None,
            }
        }
        Err(e) => DecodeResult {
            available: false,
            exact: None,
            wall_ns: Some(wall_ns),
            decoded_sha256: None,
            decoded_bytes: None,
            error: Some(e),
        },
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("sha256sum exec failed: {}", e))?;

    if !out.status.success() {
        return Err(format!("sha256sum failed rc={}", out.status));
    }

    let s = String::from_utf8(out.stdout).map_err(|e| format!("sha256sum utf8 failed: {}", e))?;
    Ok(s.split_whitespace().next().unwrap_or("").to_string())
}

fn sha256_bytes_via_tool(bytes: &[u8]) -> Result<String, String> {
    let mut p = env::temp_dir();
    let pid = std::process::id();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("time failed: {}", e))?
        .as_nanos();

    p.push(format!("nereids-sha-{}-{}.bin", pid, now));
    fs::write(&p, bytes).map_err(|e| format!("temp write failed: {}", e))?;
    let res = sha256_file(&p);
    let _ = fs::remove_file(&p);
    res
}

fn sha256_ids_u32le(ids: &[u32]) -> Result<String, String> {
    let mut data = Vec::with_capacity(ids.len() * 4);
    for id in ids {
        data.extend_from_slice(&id.to_le_bytes());
    }
    sha256_bytes_via_tool(&data)
}

fn first_divergence(a: &[u32], b: &[u32]) -> Option<(usize, Option<u32>, Option<u32>)> {
    let n = a.len().max(b.len());
    for i in 0..n {
        let av = a.get(i).copied();
        let bv = b.get(i).copied();
        if av != bv {
            return Some((i, av, bv));
        }
    }
    None
}

fn opt_bool_json(v: Option<bool>) -> String {
    match v {
        Some(true) => "true".to_string(),
        Some(false) => "false".to_string(),
        None => "null".to_string(),
    }
}

fn opt_u128_json(v: Option<u128>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "null".to_string(),
    }
}

fn opt_usize_json(v: Option<usize>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "null".to_string(),
    }
}

fn opt_str_json(v: &Option<String>) -> String {
    match v {
        Some(s) => format!("\"{}\"", json_escape(s)),
        None => "null".to_string(),
    }
}

fn write_receipt(
    args: &Args,
    fixture_bytes: &[u8],
    restored_bytes: &[u8],
    native: &TokenResult,
    nereids: &TokenResult,
    native_sha: &str,
    nereids_sha: &str,
    native_decode: &DecodeResult,
    nereids_decode: &DecodeResult,
) -> Result<(), String> {
    let fixture_sha = sha256_file(&args.fixture)?;
    let restored_sha = sha256_file(&args.du_restored)?;
    let restore_exact = fixture_bytes == restored_bytes;
    let ids_exact = native.ids == nereids.ids;
    let div = first_divergence(&native.ids, &nereids.ids);

    let first_divergence_json = match div {
        Some((i, a, b)) => format!(
            "{{\"index\":{},\"native\":{},\"nereids\":{}}}",
            i,
            a.map(|x| x.to_string())
                .unwrap_or_else(|| "null".to_string()),
            b.map(|x| x.to_string())
                .unwrap_or_else(|| "null".to_string())
        ),
        None => "null".to_string(),
    };

    if let Some(parent) = args.receipt.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create receipt parent failed: {}", e))?;
    }

    let body = format!(
        concat!(
            "{{\n",
            "  \"schema\": \"nereids-direct-id-fixture-v0\",\n",
            "  \"model_name\": \"{}\",\n",
            "  \"fixture\": \"{}\",\n",
            "  \"du_restored\": \"{}\",\n",
            "  \"native_tokenizer_server\": \"{}\",\n",
            "  \"fixture_bytes\": {},\n",
            "  \"fixture_sha256\": \"{}\",\n",
            "  \"du_restored_sha256\": \"{}\",\n",
            "  \"du_restore_exact\": {},\n",
            "  \"native_token_count\": {},\n",
            "  \"nereids_token_count\": {},\n",
            "  \"native_ids_sha256_u32le\": \"{}\",\n",
            "  \"nereids_ids_sha256_u32le\": \"{}\",\n",
            "  \"ids_exact\": {},\n",
            "  \"first_divergence\": {},\n",
            "  \"token_policy\": \"add_special=false\",\n",
            "  \"native_tokenize_wall_ns\": {},\n",
            "  \"nereids_full_window_repair_wall_ns\": {},\n",
            "  \"detokenize_available\": {},\n",
            "  \"native_decode_exact\": {},\n",
            "  \"nereids_decode_exact\": {},\n",
            "  \"native_detokenize_wall_ns\": {},\n",
            "  \"nereids_detokenize_wall_ns\": {},\n",
            "  \"native_decoded_sha256\": {},\n",
            "  \"nereids_decoded_sha256\": {},\n",
            "  \"native_decoded_bytes\": {},\n",
            "  \"nereids_decoded_bytes\": {},\n",
            "  \"native_detokenize_error\": {},\n",
            "  \"nereids_detokenize_error\": {},\n",
            "  \"notes\": [\n",
            "    \"V0 uses full-window native-tokenizer repair fallback.\",\n",
            "    \"This is a correctness receipt, not optimized DU-ID remap.\"\n",
            "  ]\n",
            "}}\n"
        ),
        json_escape(&args.model_name),
        json_escape(&args.fixture.display().to_string()),
        json_escape(&args.du_restored.display().to_string()),
        json_escape(&args.native_tokenizer_server),
        fixture_bytes.len(),
        fixture_sha,
        restored_sha,
        restore_exact,
        native.ids.len(),
        nereids.ids.len(),
        native_sha,
        nereids_sha,
        ids_exact,
        first_divergence_json,
        native.wall_ns,
        nereids.wall_ns,
        native_decode.available || nereids_decode.available,
        opt_bool_json(native_decode.exact),
        opt_bool_json(nereids_decode.exact),
        opt_u128_json(native_decode.wall_ns),
        opt_u128_json(nereids_decode.wall_ns),
        opt_str_json(&native_decode.decoded_sha256),
        opt_str_json(&nereids_decode.decoded_sha256),
        opt_usize_json(native_decode.decoded_bytes),
        opt_usize_json(nereids_decode.decoded_bytes),
        opt_str_json(&native_decode.error),
        opt_str_json(&nereids_decode.error),
    );

    fs::write(&args.receipt, body).map_err(|e| format!("write receipt failed: {}", e))?;

    Ok(())
}

fn run() -> Result<i32, String> {
    let args = parse_args()?;

    if !args.fixture.is_file() {
        return Err(format!("fixture missing: {}", args.fixture.display()));
    }
    if !args.du_restored.is_file() {
        return Err(format!(
            "du restored missing: {}",
            args.du_restored.display()
        ));
    }

    let fixture_bytes =
        fs::read(&args.fixture).map_err(|e| format!("read fixture failed: {}", e))?;
    let restored_bytes =
        fs::read(&args.du_restored).map_err(|e| format!("read du restored failed: {}", e))?;

    let fixture_text = String::from_utf8(fixture_bytes.clone())
        .map_err(|e| format!("fixture is not UTF-8: {}", e))?;
    let restored_text = String::from_utf8(restored_bytes.clone())
        .map_err(|e| format!("du restored is not UTF-8: {}", e))?;

    let native = tokenize(&args.native_tokenizer_server, &fixture_text)?;
    let nereids = tokenize(&args.native_tokenizer_server, &restored_text)?;

    let native_sha = sha256_ids_u32le(&native.ids)?;
    let nereids_sha = sha256_ids_u32le(&nereids.ids)?;

    let native_decode = detokenize(&args.native_tokenizer_server, &native.ids, &fixture_bytes);
    let nereids_decode = detokenize(&args.native_tokenizer_server, &nereids.ids, &fixture_bytes);

    write_receipt(
        &args,
        &fixture_bytes,
        &restored_bytes,
        &native,
        &nereids,
        &native_sha,
        &nereids_sha,
        &native_decode,
        &nereids_decode,
    )?;

    let restore_exact = fixture_bytes == restored_bytes;
    let ids_exact = native.ids == nereids.ids;

    println!("schema=nereids-direct-id-fixture-v0");
    println!("model_name={}", args.model_name);
    println!("fixture_bytes={}", fixture_bytes.len());
    println!("du_restore_exact={}", restore_exact);
    println!("native_token_count={}", native.ids.len());
    println!("nereids_token_count={}", nereids.ids.len());
    println!("native_ids_sha256_u32le={}", native_sha);
    println!("nereids_ids_sha256_u32le={}", nereids_sha);
    println!("ids_exact={}", ids_exact);
    println!("native_decode_exact={}", opt_bool_json(native_decode.exact));
    println!(
        "nereids_decode_exact={}",
        opt_bool_json(nereids_decode.exact)
    );
    println!("native_tokenize_wall_ns={}", native.wall_ns);
    println!("nereids_full_window_repair_wall_ns={}", nereids.wall_ns);
    println!("receipt={}", args.receipt.display());

    if restore_exact && ids_exact {
        Ok(0)
    } else {
        Ok(2)
    }
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("FAIL: {}", e);
            eprintln!("{}", usage());
            std::process::exit(1);
        }
    }
}
