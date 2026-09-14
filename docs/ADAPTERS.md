# Nereids tokenizer adapters

Nereids uses byte identity as the canonical interoperability contract.

Adapter status is deliberately conservative:

| Adapter / path | Status |
|---|---|
| Generic external JSONL native-token stream | RECEIPT-BACKED |
| DU byte/span bridge | RECEIPT-BACKED |
| Legacy cl100k/model.bin remap | LEGACY RECEIPT-BACKED |
| GPT-2 / tiktoken prototype paths | LEGACY / NEEDS NATIVE ADAPTER |
| Qwen | WANTED |
| Mistral Nemo / Tekken-v3 structural bridge | RECEIPT-BACKED |
| Mistral canonical native adapter | WANTED |
| Llama / llama.cpp | WANTED |
| DeepSeek | WANTED |
| Gemma | WANTED |
| Phi | WANTED |
| GPT-OSS | WANTED |
| Hugging Face tokenizer.json families | WANTED |
| SentencePiece families | WANTED |

Public GPT-2 and `cl100k_base` fixtures are `FIXTURE-VERIFIED` with
`tiktoken==0.14.0`.

```text
GPT-2 / tiktoken       FIXTURE-VERIFIED
cl100k_base / tiktoken FIXTURE-VERIFIED
```

Regeneration and verification:

```text
python3 -m pip install -r tools/requirements-tiktoken-fixtures.txt
python3 tools/generate_tiktoken_fixtures.py
python3 tools/generate_tiktoken_fixtures.py --verify
```

These fixtures verify the generic native-token JSONL byte contract. They are
not native Rust tokenizer adapters, and no universal tokenizer compatibility
claim is made.

## Adapter maturity

```text
EXPERIMENTAL
    ↓
CONFORMANCE PASS
    ↓
LOCAL MODEL TESTED
    ↓
RECEIPT-BACKED
```

A conformance pass means the adapter satisfies the Nereids structural
contract. It does not by itself imply that a particular production model has
been tested.

## Core adapter contract

An adapter should expose native tokens as byte-oriented records:

```text
NativeToken {
    id
    byte_start
    byte_end
    bytes
}
```

The governing invariants are:

```text
native-token reconstruction == raw bytes
DU reconstruction           == raw bytes
target-token reconstruction == raw bytes
```

Native IDs and token boundaries may differ between tokenizers.

## PRs welcome

Useful adapter contributions include:

- GPT-2 native BPE
- cl100k / tiktoken native paths
- Qwen
- Mistral
- Llama / llama.cpp
- DeepSeek
- Gemma
- Phi
- GPT-OSS
- Hugging Face tokenizer.json
- SentencePiece-based models

Adapters should remain isolated from the Nereids exact core wherever
possible.

Mock adapters are welcome for conformance testing, but must not be described
as model compatibility.

Real-model compatibility requires a local or reproducible fixture and an
exact reconstruction receipt.
