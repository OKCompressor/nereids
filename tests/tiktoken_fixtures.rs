use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct TokenRecord {
    id: u64,
    byte_start: usize,
    byte_end: usize,
    bytes_hex: String,
}

fn repo_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn load_records(name: &str) -> Vec<TokenRecord> {
    let path = repo_path(&format!("fixtures/tiktoken/{name}.jsonl"));
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));

    text.lines()
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str(line).unwrap_or_else(|err| {
                panic!(
                    "invalid JSONL record {} in {}: {err}",
                    index + 1,
                    path.display()
                )
            })
        })
        .collect()
}

fn rebuild(records: &[TokenRecord]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut cursor = 0usize;

    for record in records {
        assert_eq!(record.byte_start, cursor, "token stream must be contiguous");

        let token_bytes = hex::decode(&record.bytes_hex).expect("bytes_hex must be valid hex");

        assert_eq!(
            record.byte_end,
            record.byte_start + token_bytes.len(),
            "byte span must match token byte length"
        );

        output.extend_from_slice(&token_bytes);
        cursor = record.byte_end;
    }

    output
}

fn raw_fixture() -> Vec<u8> {
    fs::read(repo_path("fixtures/multilingual.txt")).expect("multilingual fixture must exist")
}

#[test]
fn public_gpt2_tiktoken_fixture_is_byte_exact() {
    let records = load_records("gpt2");

    assert_eq!(records.len(), 30);
    assert_eq!(rebuild(&records), raw_fixture());
}

#[test]
fn public_cl100k_fixture_is_byte_exact_and_ids_are_native() {
    let cl100k = load_records("cl100k_base");
    let gpt2 = load_records("gpt2");

    assert_eq!(cl100k.len(), 24);
    assert_eq!(rebuild(&cl100k), raw_fixture());

    let cl100k_ids: Vec<u64> = cl100k.iter().map(|record| record.id).collect();
    let gpt2_ids: Vec<u64> = gpt2.iter().map(|record| record.id).collect();

    assert_ne!(
        cl100k_ids, gpt2_ids,
        "native tokenizer ID spaces must not be conflated"
    );
}
