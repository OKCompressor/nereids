# Proteus P1 — binary checkpoint + append journal

## Baseline

P1 starts from sealed Proteus P0:

~~~text
tag:
  proteus-p0-20260915

commit:
  075f435b84faada24c09d7978d27aebe7552abc1
~~~

P0 proved:

~~~text
persist -> reload -> append -> persist -> reload -> append

candidate native IDs == full-tokenization oracle IDs

exact transcript hashes
immutable DU history
max+1 first-seen DU allocation
oracle used only after candidate construction
~~~

## P0 persistence baseline

The P0 JSON implementation remains preserved as a correctness and performance
baseline.

Observed repeated Qwen cold-resume test:

~~~text
transcript:
  approximately 6.4 KiB

snapshot:
  565166 bytes

21-run median:

Proteus JSON resume:
  5.895 ms

full native tokenize:
  2.921 ms
~~~

The result is intentionally retained even though it is slower.

It motivates changing persistence architecture rather than weakening the
exactness contract.

## P1 objective

Replace whole-snapshot JSON persistence with:

~~~text
immutable PRT0 checkpoint
+
append-only PJR0 journal
~~~

Normal appends must not rewrite historical canonical payload.

## Required implementation

Implement:

~~~text
PRT0 checkpoint encoder/loader

PJR0 section encoder/loader

payload-first layout
tail META
fixed EOF footer

content hashes
parent_record_hash
base_checkpoint_root_hash

one canonical stream head
compare-and-set head update

incremental validation
full explicit audit

checkpoint fold
~~~

JSON stays available for:

~~~text
receipts
debug export
human inspection
~~~

It is not the P1 hot persistence representation.

## Required exactness gate

For every append:

~~~text
Proteus incremental candidate native IDs
==
full native tokenizer IDs
~~~

Also require:

~~~text
exact transcript bytes/hash

all previous canonical DU IDs unchanged

new exact lexemes allocated:
  previous max canonical ID + 1
  in first-seen order

valid causal parent

valid section hashes

valid resulting state root
~~~

Oracle verification remains outside candidate construction.

## Crash/commit gate

An append becomes committed only after the final footer is complete and valid.

Tests must truncate writes at multiple offsets and prove recovery selects the
latest earlier valid footer.

Expected behavior:

~~~text
valid A
valid B
partial C

open -> B
~~~

No partial C may become canonical.

## No-history-rewrite gate

Measure:

~~~text
historical bytes read
new bytes appended
historical canonical payload bytes rewritten
~~~

Normal append target:

~~~text
historical canonical payload bytes rewritten = 0
~~~

Filesystem metadata writes are reported separately.

## Tail-discovery gate

Opening a journal must be able to discover the latest committed metadata from
EOF without scanning the complete journal from byte zero.

## Incremental-validation gate

Hot append must not require full historical reconstruction.

Full audit remains a separate command/path.

## Checkpoint-fold gate

For checkpoint C plus journal state through sequence N:

~~~text
reconstruct exact state
create checkpoint C2 @ N
verify C2
preserve transcript identity
preserve native IDs/spans
preserve every canonical DU ID
start later journal at N+1
~~~

No canonical ID remap is permitted.

## Packing gate

Logical object hashes must survive changes in physical packing.

The same logical record/section may be:

~~~text
loose
packed
merged
moved
replicated
re-indexed
~~~

without changing its identity hash.

## Benchmark matrix

Compare:

~~~text
P0 whole-JSON snapshot
P1 PRT0 + PJR0
full native retokenization
~~~

at:

~~~text
4 KiB
64 KiB
1 MiB
~~~

Separate measurements for:

~~~text
checkpoint open
journal-tail discovery
incremental validation
DU delta reconstruction
Direct-ID candidate preparation
span delta construction
dictionary delta construction
journal encoding
journal append
footer commit

full audit

native tokenizer oracle

model inference
~~~

Do not combine model-inference latency with persistence/tokenizer timing.

## Planned receipt timings

At minimum:

~~~text
checkpoint_open_wall_ns
journal_tail_discovery_wall_ns
incremental_validation_wall_ns

delta_du_reconstruct_wall_ns
direct_id_candidate_prep_wall_ns
native_span_delta_wall_ns
dictionary_delta_wall_ns

journal_encode_wall_ns
journal_append_wall_ns
footer_commit_wall_ns

proteus_p1_append_total_wall_ns

oracle_full_tokenize_wall_ns
~~~

## Roadmap boundary

P1:

~~~text
binary checkpoint
append-only journal
EOF metadata/footer
incremental validation
one canonical head
checkpoint folding
~~~

P1.x:

~~~text
journal segment packing
chunk merge/split
derived rengram/transducer indexes
diff/object packs
partial physical views
~~~

P2:

~~~text
content-addressed replicated object DAG
signed heads/refs
P2P object exchange
gossip / causal knowledge propagation
partial federated views
~~~

Later, if required:

~~~text
multi-authority federation
Byzantine/quorum validation
semantic multi-parent interaction merge
ordering policies for concurrent interaction streams
~~~

No later roadmap item is required to prove P1.

## Contract

See:

[`docs/contracts/proteus-p1-storage.md`](../contracts/proteus-p1-storage.md)

## Boundary to Prometh runtime state

Proteus owns exact archived conversation state.

Prometh is the later model-runtime state layer.

Conceptually:

~~~text
large Proteus archive
        |
        v
selected bounded active view
        |
        v
exact native IDs
        |
        v
Prometh runtime continuation state
        |
        v
model
~~~

The intended future optimization is that a valid persisted runtime state can
allow continuation from an already processed active prefix without replaying
that whole prefix for every new turn.

The strongest exact future case is model-specific KV/prefix state.

Exact KV reuse requires compatibility including at least:

~~~text
same model architecture
same weights
same exact native token prefix
same chat/rendering policy
same positional state
same relevant runtime policy
compatible KV representation/backend
~~~

When those conditions hold and the runtime exposes sufficient state, a future
Prometh append may approach:

~~~text
load compatible continuation state
+
process only newly appended model tokens
~~~

instead of:

~~~text
re-tokenize archive
+
prefill complete active prefix
+
process new tokens
~~~

Proteus P1 does not yet implement this.

A large archived conversation therefore does not imply that all archived
tokens are simultaneously resident in model context.

If the active view changes and no compatible saved runtime state exists for
the required prefix, the newly selected prefix must be prefetched/recomputed
or loaded from another compatible state checkpoint.

Generic hidden-state or learned cross-model projections are not considered
exact continuation state unless separately proven.

Future learned projection lanes remain approximate by default.

## Scale ladder

In addition to the original P1 sizes, the large-history lane should eventually
measure:

~~~text
4 KiB
64 KiB
1 MiB
10 MiB
100 MiB
1 GiB
~~~

using a bounded-size append/delta.

The hypothesis under test is:

~~~text
Proteus append work
approximately follows:
  new delta
  + bounded tokenizer repair
  + bounded/index update work

rather than:
  complete archived-history size
~~~

This is a benchmark hypothesis, not a claim until receipts exist.

Model inference timing remains a separate lane because without compatible
runtime-state reuse, transformer prefill cost still depends on the active
model context.
