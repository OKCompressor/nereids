# Direct-ID Adapter V0 fixture receipt — 2026-09-15

## Result

Direct-ID V0 uses a full-window native-tokenizer repair fallback.
This is a correctness adapter, not the optimized runtime path.

| Model | Bytes | Native IDs | DU restore | IDs exact | Native decode | Nereids decode | Native tokenize ms | Full repair ms |
|---|---:|---:|---|---|---|---|---:|---:|
| Qwen3.5-9B Q4_K_M | 4096 | 1485 | True | True | True | True | 1.560 | 2.182 |
| Mistral-Nemo Q3_K_L | 4096 | 1434 | True | True | True | True | 5.527 | 4.768 |

## Exactness

### Qwen3.5-9B Q4_K_M

- Native/Nereids ID SHA-256: `ef697c4ad2c7d9f9a8590c89b0c091cc4edce3ee8f2ac0bf8c5cf094e212842f`
- Exact native ID equality: `true`
- Exact DU byte restoration: `true`

### Mistral-Nemo Q3_K_L

- Native/Nereids ID SHA-256: `077cceb1a8a8613416ef535746e79c68f665567f10a6cee6f0b97d1381ae88a0`
- Exact native ID equality: `true`
- Exact DU byte restoration: `true`

## Receipt

- Summary SHA-256: `e130cce0fe150fb8c9e3b4911591ac2a18e6f650a0fed1d11bca5823fc6bf0a8`
- Receipt directory: `/mnt/data_linux/IT/OKC/research-private/nereids-rs/receipts/direct-id-v0-fixture-20260915-170556`

## Claim boundary

This receipt proves:

~~~text
DU-restored bytes
-> full-window native repair
-> exact native model IDs
~~~

It does not yet prove:

~~~text
DU IDs -> native IDs without full-window tokenizer fallback
bounded repair-tail correctness
tokenization latency improvement
better model quality
~~~

The next implementation gate replaces full-window repair with a
bounded repair region while retaining the same native-ID equality gate.
