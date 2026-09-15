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

## Distinct benchmark lanes

These three lanes answer different questions and must not be combined into
one workload claim.

### 1. Fixed-delta depth microstress

Repeat one exact delta against the evolving state at:

    1 / 10 / 100 / 1k / 10k / 100k sequential appends

This isolates turn-depth/state-growth behavior. Repetition does not model
diverse conversation content.

### 2. Manifest-driven sequential replay

Replay a deterministic manifest of delta byte files, exact DU delta runs and
source labels. Duplicate payload bytes are legal logical events; workload
diversity is a property of the selected dataset, not an invariant enforced by
replay. Load checkpoint/journal and workload files once, keep one session in
memory, and mutate it sequentially without cloning full state per turn.
Construct each candidate, transcript identity and logical head before any
native-oracle call.

Use `--oracle-every 1` for the certification lane. Scale/performance runs may
use periodic checks (`--oracle-every N`), per-turn `force_oracle`, and the
default final check. `--oracle-every 0 --no-final-oracle` makes no scheduled or
automatic-final calls, although forced turns remain checked. Only a receipt
whose every turn was actually oracle checked and matched may emit
`ALL_TURNS_EXACT: true`.

### 3. Archive-size / Hutter scaling

Measure deterministic long-input scaling over:

| Corpus | Exact bytes | SHA256 |
|---|---:|---|
| enwik5 | 100,000 | 581bee9758c531de3795885a1adb331b9c51bfdeefd9a63407815479e22a9072 |
| enwik6 | 1,000,000 | 369b688978f649681136198fb96db14c1616756260c55fb4b65e9bc049552cad |
| enwik7 | 10,000,000 | 5985c81c39d927ae0e169625790ca4d9e7d1531270c8b09ad73176a375bb3d97 |
| enwik8 | 100,000,000 | 2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8 |
| enwik9 | 1,000,000,000 | 159b85351e5f76e60cbe32e04c677847a9ecba3adc79addab6f4c6c7aa3744bc |

Hutter enwik5 through enwik9 are deterministic long-input stress corpora.
They are not conversation data and results from this lane are not
conversation-workload claims.

## Raw-byte ingest boundary

Raw bytes -> canonical DU -> native adapter IDs/spans -> immutable Proteus
checkpoint is general Proteus functionality, not benchmark-only code. P1
exposes `create_checkpoint_from_exact_bytes` with explicit exact
`TokenizerStream`, `LoadedStream`, model/tokenizer/provenance identity and DU
lineage inputs. The `checkpoint-from-bytes` CLI consumes the corresponding
native JSONL and existing Nereids DU-run artifacts. Proteus validates those
artifacts but does not silently invoke providers, generate DUs or normalize
the transcript.

A future large-container implementation may use immutable logical chunks plus
small authenticated indexes and range reads. Logical object identity must
remain independent of physical file offsets or packing.

P1 does not physically deduplicate duplicate delta payloads or encode REF
records. Content-addressed physical deduplication/reference packing is
compatible with a future container or P1.x because sequence, parent/head,
transcript position and provenance—not payload storage location—define each
logical event.

## Claim boundary

Exact bytes, spans, reconstruction and verified native-ID parity are exact.

Performance claims apply only to the named receipts and lanes. Learned model
state, KV projection and cross-model continuation are outside Proteus P1.

## Oracle and workload policy

An `ALL_TURNS_EXACT: true` result requires native-oracle verification on every
measured turn unless exactness has been established by an independent proof.

A final-only or sampled oracle is insufficient to claim that every
intermediate native-ID state was exact, because subsequent state evolves from
the prior accepted state.

The current configurable receipt states only the exact set and count of turns
that were oracle verified. A later complete offline verification pass could
certify an immutable replay receipt; that verifier is outside this P1 change.

The repeated `word64` sequential lane is the controlled fixed-delta
turn-depth microstress above. Any diverse-turn claim belongs to the selected
manifest dataset, not to replay mechanics.

## 10,000-turn exact depth result

Receipt:

    receipts/proteus-p1-qwen-true-warm-10k-20260915-222957/receipt.json

Receipt SHA256:

    02670c74ed1faf2b8acd77e11e97ca88ba20f111a02b2d738c009b049182c7f2

This is a fixed-delta turn-depth microstress, not a natural-conversation
benchmark.

The initial transcript was 6,400 bytes. The same exact 64-byte test delta was
then appended 10,001 times in one in-memory Proteus session. Final transcript
size was 646,464 bytes.

Every turn was verified against the Qwen native tokenizer oracle:

    ALL_10001_IDS_EXACT=true
    TRUE_WARM_10K_EXACT=PASS

| Window | History | Full tokenizer | Native-ID | Validation | Warm total | Warm speedup |
|---|---:|---:|---:|---:|---:|---:|
| 2-101 | 9,696 B | 5.468 ms | 0.623 ms | 0.710 ms | 1.418 ms | 3.86x |
| 902-1001 | 67,296 B | 32.957 ms | 1.062 ms | 1.717 ms | 2.904 ms | 11.35x |
| 4902-5001 | 323,296 B | 198.024 ms | 3.110 ms | 6.533 ms | 9.957 ms | 19.89x |
| 9902-10001 | 643,296 B | 469.257 ms | 6.777 ms | 15.722 ms | 23.255 ms | 20.18x |
| all 10k | 326,496 B | 209.784 ms | 3.284 ms | 7.190 ms | 10.811 ms | 19.40x |

Peak fixture-process RSS:

    27,276 KiB
    26.6 MiB

At the late window, incremental validation is the dominant measured component
of warm compute. This identifies validation as the next optimization target;
it does not establish that all of that work can safely be removed.

## Exact raw-byte ingest and diverse replay smoke

Receipt:

    receipts/proteus-p1-night-smoke-20260915-230246/replay.json

Receipt SHA256:

    02418ff77bd77e51b25305f0e7e37f1e65ced2b974becd1f822c744fc6e61601

The smoke imported the exact 6,400-byte natural prefix with
`checkpoint-from-bytes`, then replayed two distinct deltas in one detached
in-memory session:

    turn 1: word64, 64 bytes
    turn 2: UTF-8 ñ, 2 bytes

Both turns were oracle checked. Native IDs, native spans and DU lineage were
exact. Candidate state was constructed before the oracle. The journal was not
mutated.

    ALL_TURNS_EXACT=true

## Frozen P1 baseline backup

Implementation baseline commit:

    481595fd6e30afecc54f325c2f6f637976e9727f

Annotated tag:

    proteus-p1-ingest-replay-20260915
    tag object a7650b3a56ee91190f297c6ef210cae518ed7bab

Bundle:

    /mnt/data_linux/IT/OKC-releases/proteus-p1-ingest-replay-20260915-481595f.bundle

Bundle SHA256:

    8fd40dd4be02cd97aedf82ad665b84f410621013abc119b1f37e5fec360d5dbd

The bundle verifies as a complete Git history.

## Next measured lanes

The next turn-depth workload should use distinct deterministic payloads with a
controlled size distribution. Workload diversity is a dataset property, not a
Proteus replay invariant.

Archive-size scaling uses the exact Hutter ladder:

    enwik5      100,000 bytes
    enwik6    1,000,000 bytes
    enwik7   10,000,000 bytes
    enwik8  100,000,000 bytes
    enwik9 1,000,000,000 bytes

Measurements should keep native-ID construction, incremental validation, full
warm compute, reopen cost, bytes read, checkpoint size and peak RSS separate.
