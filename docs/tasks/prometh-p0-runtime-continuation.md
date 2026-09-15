# Prometh P0 — exact runtime continuation state

## Starting point

Proteus P1 is frozen as the exact archive/state layer.

Qwen fixed-delta depth proof:

- 10,001 / 10,001 turns exact against the native tokenizer oracle.
- Final transcript: 646,464 bytes.
- Peak Proteus fixture RSS: 26.6 MiB.
- Late window:
  - full native tokenizer: 469.257 ms
  - Proteus native-ID construction: 6.777 ms
  - Proteus full warm compute: 23.255 ms
  - warm ratio vs full tokenization: 20.18x

Incremental validation still contains history-wide work and is a known
optimization opportunity. It is deliberately deferred because P1 already
establishes the required exactness and scaling direction.

Prometh now targets model inference/repeated prefill, not tokenizer work.

## P0 question

Given an already processed exact native-token prefix, can a compatible model
runtime persist and restore continuation state so that the next request
processes only the newly appended or changed suffix rather than prefilling the
complete active prefix again?

## Exact continuation boundary

An exact restored state requires at least:

- identical model architecture;
- identical model weights;
- identical exact native-token prefix;
- identical chat rendering and special-token policy;
- compatible positional state;
- compatible runtime/backend policy;
- compatible KV representation;
- all runtime metadata required for deterministic continuation.

A state that does not satisfy this compatibility contract is not called exact.

Cross-model projection, learned compression and generic hidden-state transfer
are outside P0 and are approximate unless separately proven.

## Proteus / Prometh division

Proteus owns durable conversation truth:

    exact bytes
    canonical DU structure
    exact native IDs/spans
    provenance
    causal heads
    arbitrarily large archive

Prometh owns model continuation state for a bounded active view:

    compatible KV/prefix state
    position/runtime metadata
    model/runtime identity
    active-prefix identity

Archive size and active inference-state size are separate axes.

A 1 GB Proteus archive does not imply a 1 GB active model context.

Historical material outside the current active view may later be selected from
Proteus and reintroduced. If that changes the active prefix, Prometh may reuse
only an exact compatible checkpoint/prefix and must recompute the changed
suffix.

## First benchmark lanes

### A. Direct

    full active prompt
    -> native tokenize
    -> full model prefill
    -> new-token decode

### B. Proteus only

    Proteus exact native IDs
    -> full model prefill
    -> new-token decode

This isolates tokenizer/state-orchestration savings.

### C. Prometh warm

    compatible KV already resident
    -> append new exact native IDs
    -> process only new suffix
    -> decode

### D. Prometh reopen

    restore compatible persisted runtime state
    -> append new exact native IDs
    -> process only new suffix
    -> decode

This measures persisted-state load cost against repeated full prefill.

## Required measurements

Keep separate:

- active prefix token count;
- new suffix token count;
- native-ID preparation;
- full prefill wall time;
- KV/state save wall time;
- KV/state restore wall time;
- bytes persisted/read;
- peak RSS;
- first-token latency;
- generation time;
- total request time.

Do not fold generation throughput into tokenizer or state-restore claims.

## Scale

Initial active-context ladder should be based on what the frozen local model
and runtime actually support.

Example measurement points:

    4k
    16k
    32k
    larger supported contexts

Proteus archive size is tested independently:

    100 KB
    1 MB
    10 MB
    100 MB
    1 GB

## Checkpoint topology

P0 should not assume that every token receives a persisted checkpoint.

Measure:

- checkpoint per completed turn;
- checkpoint every K tokens;
- recent KV resident in RAM;
- older compatible checkpoints persisted.

Branching should reuse the longest exact compatible prefix.

Checkpoint identity should eventually bind:

    Proteus logical head / active-view identity
    + exact native-prefix identity
    + model identity
    + runtime compatibility identity

Physical storage location is not logical identity.

## Deferred Proteus optimization

Proteus warm validation currently performs history-scaled work including full
DU/native/provenance/dictionary checks and lookup reconstruction.

This is recorded as a future optimization target.

Do not reopen it during Prometh P0 unless measurements show it materially
blocks inference-side experiments.

## P0 success

Prometh P0 succeeds only with receipts demonstrating that a restored
compatible runtime state produces the same deterministic continuation as the
corresponding uninterrupted/full-prefill baseline under frozen model/runtime
conditions.

No end-to-end speed claim is made before those receipts exist.

## Sprint focus

Proteus P1 optimization is intentionally deferred.

Known future Proteus work includes removing history-wide validation from the
warm append path and, if scale receipts require it, replacing whole-state
materialization with indexed/range-loaded immutable chunks.

Those changes are not prerequisites for Prometh P0. The measured Proteus P1
path already establishes exact incremental state advancement and a clear
advantage over full retokenization at the tested Qwen history sizes.

Prometh P0 now focuses exclusively on inference-side continuation:

    exact Proteus active-prefix identity
    -> compatible saved model runtime state
    -> restore state
    -> process only new/changed suffix
    -> deterministic continuation equivalence

## Future physical packing

Proteus logical state and Prometh runtime-state objects must remain independent
from physical packing.

Future OKC/container integration may losslessly:

- chunk;
- compress;
- deduplicate;
- content-address;
- pack;
- range-load;
- relocate

Proteus transcript/DU/native state and opaque Prometh runtime-state blobs
without changing their logical identities.

For Prometh, a physical KV/state blob is not self-describing proof of
compatibility. Its descriptor must bind at least the exact native prefix,
model/weights identity, runtime compatibility identity, positional state and
KV/state representation required for restoration.

Lossy compression, cross-model projection and learned state conversion are not
part of the exact P0 lane.
