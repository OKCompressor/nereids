# Nereids 0.13.37-alpha.1

Rust revival of the legacy Nereid byte/span bridge.

## Proven in the current Rust implementation

- release binary builds
- CLI help and doctor pass
- byte reconstruction smoke is exact
- DU lexeme decoding is ported
- DU/target byte-span ledger is ported
- generic native-token JSONL bridge is implemented
- source, DU, and target streams are validated against canonical raw bytes
- gaps, overlaps, incorrect token bytes, and malformed lexemes are rejected
- differing tokenizer boundaries can reconstruct identical bytes
- multibyte UTF-8 is handled using byte offsets
- target tokens crossing multiple DU spans are measured
- Phase 2 + public tiktoken fixture test suite passes 16/16

## Current exactness boundary

```text
bytes / spans / verified tokenizer reconstruction
    = EXACT

embedding / hidden / KV projection
    = LEARNED + APPROXIMATE
```

## Separately measured DU integration receipt

Fresh DU3-v1g Hutter enwik8 run:

- input bytes: 100000000
- DU encode: 0.96 s
- merge: 0.19 s
- gmap32: 0.25 s
- structural total: approximately 1.40 s
- restore: 0.17 s
- exact roundtrip: true
- input/output SHA256:
  2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8

## Not claimed

- universal tokenizer compatibility
- native Qwen/Mistral/Llama/DeepSeek compatibility until adapter receipts exist
- faster native BPE/tokenization until same-input benchmarks exist
- faster LLM inference
- exact cross-model KV-cache portability
- exact embedding or hidden-state projection
- native-ID-equivalent incremental/delta tokenization yet
- PLM training or PLM compatibility
