# Nereids

**Byte-exact tokenizer interoperability through a shared structural byte/span layer.**

Nereids is a Rust research module for moving exact byte structure between
heterogeneous tokenizer spaces without pretending that tokenizer IDs are
portable across models.

The core rule is simple:

```text
different token IDs
different token boundaries
same exact bytes
```

Nereids treats exact structural reconstruction separately from learned model
state.

```text
bytes / spans / verified reconstruction
    = EXACT

embeddings / hidden states / KV projection
    = LEARNED + APPROXIMATE
```

## Current status

`0.13.37-alpha.1`

Implemented and tested:

- exact byte reconstruction
- DU-style byte/span alignment
- generic native-token JSONL streams
- preservation of external/native token IDs
- detection of gaps, overlaps, malformed spans, and incorrect token bytes
- UTF-8-safe byte offsets
- target tokens crossing multiple structural spans
- public GPT-2 and `cl100k_base` tiktoken fixtures
- deterministic fixture regeneration and verification

The checked-in tiktoken fixtures use the same 70-byte multilingual input:

```text
GPT-2        30 tokens
cl100k_base  24 tokens
tiktoken     0.14.0
rebuild      exact
```

These fixtures verify Nereids' generic byte/token contract. They are **not**
native Rust tokenizer implementations.

## Quick start

```bash
cargo test --all-targets
cargo build --release

./target/release/nereids doctor
./target/release/nereids --help
./target/release/nereids bridge-jsonl --help
```

To verify the public tiktoken fixtures:

```bash
python3 -m venv /tmp/nereids-tiktoken
/tmp/nereids-tiktoken/bin/pip install \
  -r tools/requirements-tiktoken-fixtures.txt

/tmp/nereids-tiktoken/bin/python \
  tools/generate_tiktoken_fixtures.py --verify
```

## Architecture

```text
source tokenizer
      │
      ▼
native token IDs + exact token bytes
      │
      ▼
byte spans
      │
      ▼
shared structural representation
      │
      ▼
byte spans
      │
      ▼
target tokenizer/native token view
```

Native token IDs are local to their tokenizer. Nereids does not require IDs or
token boundaries to match.

The invariant is exact reconstruction of the canonical byte stream.

## Adapter maturity

Adapters and integrations should progress through explicit evidence levels:

```text
EXPERIMENTAL
    ↓
CONFORMANCE PASS
    ↓
LOCAL MODEL TESTED
    ↓
RECEIPT-BACKED
```

See [`docs/ADAPTERS.md`](docs/ADAPTERS.md).

Contributions for additional tokenizer families are welcome, including:

- Qwen
- Mistral
- Llama / llama.cpp
- DeepSeek
- Gemma
- Phi
- GPT-OSS
- SentencePiece
- Hugging Face `tokenizer.json` families

Your tokenizer, your adapter, same exact byte contract.

## What Nereids does not claim

Nereids currently does **not** claim:

- universal tokenizer compatibility
- faster LLM inference
- exact cross-model KV-cache portability
- exact embedding or hidden-state projection
- native-ID-equivalent incremental tokenization
- production compatibility with a model family without adapter receipts

Performance claims belong to reproducible benchmarks, not architecture
diagrams.

## Provenance

Nereids revives and restructures earlier Nereid tokenizer/DU experiments.
Legacy Python prototypes are retained under [`legacy/python/`](legacy/python/)
for lineage and comparison.

Generated receipts and large benchmark artifacts are intentionally excluded
from the source tree.

## License

Nereids is **source-available under the Luna Non-Commercial License 1.2
(LNCL 1.2)**.

This is not an OSI open-source license.

See [`LICENSE.md`](LICENSE.md).
