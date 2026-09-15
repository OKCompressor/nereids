# Proteus P1 — Qwen exact warm benchmark

Date: 2026-09-15

## Scope

This benchmark separates three different costs:

- `native-ID construction`: DU/native-span state plus bounded repair producing
  the exact native tokenizer IDs.
- `warm compute`: complete same-process in-memory Proteus state advancement.
- `reopen + append`: process restart, checkpoint/journal recovery and persisted
  append.

Warm compute contains no checkpoint/journal I/O and excludes the native
tokenizer oracle. The candidate and new logical head are complete before the
oracle is invoked.

The native tokenizer oracle is verification, not part of Proteus warm timing.

## Exactness

Qwen adapter policy for these fixtures: fixed repair depth 1.

101-turn warm receipt:

    receipts/proteus-p1-qwen-true-warm-20260915-221154/receipt.json
    SHA256 469c3290f149d143784b80bcd25335dc6e82e6fe0eafa41e5a68b5821a45bba7

1001-turn warm receipt:

    receipts/proteus-p1-qwen-true-warm-1k-20260915-222411/receipt.json
    SHA256 ccc2eb68430c1e419af25f7073453a25414db3949a87176eb4d55d1a400572d6

The 1001-turn run reports:

- all native IDs exact against the native tokenizer oracle;
- oracle used only for verification;
- zero filesystem I/O inside the warm loop;
- zero journal mutation;
- 6,400 -> 70,464 transcript bytes;
- peak fixture-process RSS: 5.5 MiB.

## 101-turn headline

| Lane | Native tokenizer | Proteus | Result |
|---|---:|---:|---:|
| Native-ID construction | 3.789 ms | 0.568 ms | 6.67x / 85.0% less |
| Full warm compute | 3.789 ms | 1.274 ms | 2.97x / 66.4% less |

The first sample is excluded from headline warm medians.

## 1000 measured sequential appends

| Window | Median history | Full tokenizer | Native-ID | Validation | Warm total | Warm speedup |
|---|---:|---:|---:|---:|---:|---:|
| early 2-101 | 9,696 B | 5.319 ms | 0.637 ms | 0.744 ms | 1.482 ms | 3.59x |
| middle 452-551 | 38,496 B | 18.538 ms | 0.785 ms | 1.334 ms | 2.195 ms | 8.45x |
| late 902-1001 | 67,296 B | 33.372 ms | 0.997 ms | 1.608 ms | 2.753 ms | 12.12x |
| all 1000 | 38,496 B | 18.571 ms | 0.819 ms | 1.208 ms | 2.154 ms | 8.62x |

All-1000 native-ID median:

- 0.819 ms Proteus;
- 18.571 ms full native tokenizer;
- 22.68x ratio;
- 95.6% tokenizer work avoided.

All-1000 full warm median:

- 2.154 ms;
- 8.62x ratio against full native tokenization;
- 88.4% lower measured wall time;
- p90 2.945 ms;
- p99 3.869 ms.

These are preprocessing/state-advancement measurements. They are not model
generation or end-to-end inference speedups.

## Reopen reality

31-process reopen benchmark:

    receipts/proteus-p1-qwen-reopen-bench-20260915-215433/summary.json
    SHA256 63cb969725bdb26046973cf74cf371d944711645ed4b6fb9216cecff9b0c29e8

| Lane | Turn 1 | Turn 2 |
|---|---:|---:|
| Full native tokenizer | 3.218 ms | 2.760 ms |
| Proteus native-ID construction | 0.817 ms | 0.663 ms |
| Proteus reopen + append | 6.034 ms | 5.523 ms |
| Bytes read | 163,077 B | 168,364 B |
| Historical canonical bytes rewritten | 0 B | 0 B |

At this small history, process/storage lifecycle dominates reopen cost.

## Scale lanes

Turn-depth stress and archive-size stress are separate experiments.

Turn-depth:

    1 / 10 / 100 / 1k / 10k / 100k sequential appends

Archive-size:

| Corpus | Exact bytes | SHA256 |
|---|---:|---|
| enwik5 | 100,000 | 581bee9758c531de3795885a1adb331b9c51bfdeefd9a63407815479e22a9072 |
| enwik6 | 1,000,000 | 369b688978f649681136198fb96db14c1616756260c55fb4b65e9bc049552cad |
| enwik7 | 10,000,000 | 5985c81c39d927ae0e169625790ca4d9e7d1531270c8b09ad73176a375bb3d97 |
| enwik8 | 100,000,000 | 2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8 |
| enwik9 | 1,000,000,000 | 159b85351e5f76e60cbe32e04c677847a9ecba3adc79addab6f4c6c7aa3744bc |

Hutter/enwik is deterministic long-input stress, not a natural-conversation
benchmark.

## Planned production boundary

Raw bytes -> canonical DU -> native adapter IDs/spans -> immutable Proteus
checkpoint is a general Proteus ingest capability, not a benchmark-only
fixture.

A future large-container implementation may use immutable logical chunks plus
small authenticated indexes and range reads. Logical object identity must
remain independent of physical file offsets or packing.

## Claim boundary

Exact bytes, spans, reconstruction and verified native-ID parity are exact.

Performance claims apply only to the named receipts and lanes. Learned model
state, KV projection and cross-model continuation are outside Proteus P1.

## Oracle and workload policy

An `ALL_TURNS_EXACT` result requires native-oracle verification on every
measured turn unless exactness has been established by an independent proof.

A final-only or sampled oracle is insufficient to claim that every
intermediate native-ID state was exact, because subsequent state evolves from
the prior accepted state.

The repeated `word64` sequential lane is a controlled fixed-delta
turn-depth microstress. It measures state-growth, validation, timing and RSS
behavior. It is not a natural-conversation or diverse-content benchmark.

A separate diverse-turn replay lane will use distinct deterministic payloads
and native-oracle verification on every turn.
