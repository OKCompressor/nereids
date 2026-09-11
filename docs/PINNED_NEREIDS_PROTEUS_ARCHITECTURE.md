# Nereids / Proteus — Pinned Architecture

Status: long-term north star.

## Exactness boundary

```text
bytes / spans / tokenizer reconstruction
    = EXACT

embedding / hidden / KV projection
    = LEARNED + APPROXIMATE
```

These claims must never be mixed.

## Nereids / Proteus

```text
                 NEREIDS / PROTEUS

raw I/O
  ↓
DU canonical byte graph
  ↓
conversation snapshots
  ↓
delta overlays
  ↓
┌─────────────────────────────────────────┐
│ exact tokenizer adapters                │
│                                         │
│ DU ↔ GPT2 / cl100k                      │
│ DU ↔ Qwen                               │
│ DU ↔ Mistral                            │
│ DU ↔ Llama / llama.cpp                  │
│ DU ↔ future tokenizer spaces            │
└─────────────────────────────────────────┘
  ↓
native target IDs
  ↓
native model
```

## Exact model-I/O bridge

```text
Model A output bytes
      ↓
native tokenizer A
      ↓
(source IDs, token bytes, byte spans)
      ↓
Nereids
      ↓
DU structural representation
      ↓
(target IDs, token bytes, byte spans)
      ↓
native model B input
```

Required invariant:

```text
source bytes
    ==
source-token reconstruction
    ==
DU reconstruction
    ==
target-token reconstruction
```

Native IDs are tokenizer-specific and need not be equal.

## Snapshot / delta path

```text
S0 = DU(conversation prefix)

S1 = S0 ⊕ D1
S2 = S1 ⊕ D2
S3 = S2 ⊕ D3
```

Target long-context path:

```text
large stable prefix
       +
small new turn
       ↓
DU snapshot + delta
       ↓
tokenizer boundary repair
       ↓
native target IDs
```

Strong gate:

```text
native_full_tokenize(prefix || delta)
        ==
nereids_incremental_native_ids(prefix, delta)
```

If exact ID equality holds, only the changed tail and necessary repair
window should need structural work.

## Model views

```text
                   ┌─ Qwen native view ───→ Qwen runtime
DU conversation ───┼─ Mistral native view → Mistral runtime
                   └─ Llama native view ──→ Llama runtime
```

A model-specific KV cache is not assumed portable to another model.

## Proteus learned layer

Future research:

```text
DU structural state
        ↓
learned projector / adapter
        ↓
embedding / hidden / KV initialization
        ↓
native model
```

This layer is approximate.

```text
DU bytes / spans              EXACT
tokenizer reconstruction      EXACT when verified

DU → embeddings               APPROXIMATE
DU → hidden state             APPROXIMATE
DU → foreign-model KV         APPROXIMATE
```

Possible form:

```text
frozen base model

+ exact Nereids structural snapshot
+ small learned Proteus projector
+ ephemeral conversation delta
```

## PLM / OKC boundary

Nereids / Proteus does not require redesigning present transformer
architectures.

Future PLM/OKC work may investigate models that natively consume persistent
structured context, portable state, delta state, or non-BPE representations.

That remains a separate module and claim surface.

## Current exact receipt

Fresh Hutter enwik8:

```text
input_bytes=100000000

encode_wall_sec=0.96
merge_wall_sec=0.19
gmap32_wall_sec=0.25
structural_encode_total_sec≈1.40

restore_wall_sec=0.17

input_sha256=
2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8

output_sha256=
2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8

exact=true
```

Receipt root:

```text
/home/ace/Projects/_work/okc-worker/jobs/
nereids-du3-v1g-enwik8-20260911-133013
```

This demonstrates fast byte-exact DU structural processing.

It does not by itself demonstrate faster native tokenization, faster LLM
inference, portable KV caches, or learned latent-state transfer.

## North star

Nereids makes canonical conversational state independent of any one model's
tokenizer while exposing exact reproducible native-token views.

Proteus adds persistent snapshots, deltas, model-I/O orchestration, and later
experimental learned projections without weakening the exact structural core.
