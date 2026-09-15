# Task: Direct-ID V2 DU-aware snapshot repair

## Goal

Extend Direct-ID V1 so append repair can use persisted snapshot byte-span
metadata and DU structural state.

V2 must preserve exact canonical native model IDs.

It is still pre-Proteus.

## Starting point

Direct-ID V1 already proves:

~~~text
native snapshot IDs
+ appended bytes
-> preserve stable native-ID prefix
-> retokenize adaptive suffix + delta
-> splice
-> exact native IDs
~~~

Acceptance receipt:

~~~text
144 / 144 exact
Qwen3.5 max observed minimum drop: 1 token
Mistral-Nemo max observed minimum drop: 2 tokens
~~~

V1 still uses detokenization during boundary discovery.

V2 removes that requirement from candidate construction.

## Architecture

Extend `src/direct_id.rs`.

Add explicit snapshot structures representing:

~~~text
prefix byte length/hash
native token IDs
native token byte spans
DU structural state sufficient to recover exact bytes
~~~

Suggested shape, adapted to existing project types where appropriate:

~~~text
DirectIdSnapshotV2
  prefix_len
  prefix_sha256
  native_ids
  native_spans
  du_state
~~~

Do not duplicate an existing Nereids span type if `TokenSpan`,
`TokenizerStream`, or another existing structure already expresses the
required invariant.

## Required native span invariant

For every native token:

~~~text
span.byte_start < span.byte_end
spans are contiguous
first span starts at 0
last span ends at prefix byte length
span bytes reconstruct prefix exactly
span token ID matches stored native ID
~~~

The V2 candidate path must use these stored spans to locate the repair start.

It must not detokenize the full retained native-ID prefix merely to discover
its byte boundary.

## DU-aware delta invariant

The delta presented to candidate construction must have DU provenance.

Candidate repair bytes must be recoverable exactly from DU structure using
existing Nereids DU dictionary/stream facilities.

Reuse existing DU loaders and lexeme reconstruction functions.

Do not invent a second DU serialization if the repository already has a
usable representation.

At the DU boundary:

~~~text
DU IDs / DU structural stream
-> canonical DU lexeme bytes
-> exact delta bytes
~~~

must be verified byte-exact.

## Candidate algorithm

Inputs conceptually:

~~~text
DirectIdSnapshotV2
DU-origin delta state
max_drop_tokens
native tokenize callback
~~~

For each candidate `drop_tokens`:

1. Determine kept native-ID count.
2. Read repair-start byte directly from stored native token spans.
3. Recover exact snapshot-tail bytes from snapshot/DU state.
4. Recover exact appended delta bytes from DU state.
5. Construct only:

~~~text
repair_tail_bytes || delta_bytes
~~~

6. Native-tokenize that repair window only.
7. Splice:

~~~text
kept_snapshot_native_ids || repaired_native_ids
~~~

Candidate construction must never native-tokenize:

~~~text
full_prefix || delta
~~~

## Oracle

Full native tokenization remains permitted only as fixture verification:

~~~text
oracle_ids = native_tokenize(prefix || delta)
candidate_ids == oracle_ids
~~~

Oracle work must remain separate in receipts and timings.

## Required V2 receipt fields

Include at least:

~~~json
{
  "schema": "nereids-direct-id-du-aware-v2",
  "model_name": "...",

  "prefix_bytes": 0,
  "delta_bytes": 0,
  "full_bytes": 0,

  "prefix_sha256": "...",
  "delta_sha256": "...",
  "full_sha256": "...",

  "du_delta_reconstruction_exact": false,

  "native_snapshot_token_count": 0,
  "native_snapshot_span_count": 0,
  "native_snapshot_spans_exact": false,

  "drop_tokens": 0,
  "repair_start_byte": 0,
  "snapshot_tail_bytes": 0,
  "repair_window_bytes": 0,

  "candidate_token_count": 0,
  "oracle_token_count": 0,

  "candidate_ids_sha256_u32le": "...",
  "oracle_ids_sha256_u32le": "...",

  "ids_exact": false,
  "decode_exact": false,

  "candidate_used_full_window": false,
  "candidate_used_full_prefix_detokenize": false,
  "oracle_used_for_verification_only": true,

  "du_reconstruct_wall_ns": 0,
  "span_lookup_wall_ns": 0,
  "repair_tokenize_wall_ns": 0,
  "splice_wall_ns": 0,
  "candidate_total_prep_wall_ns": 0,
  "oracle_full_tokenize_wall_ns": 0
}
~~~

## Timing requirement

V2 must finally measure candidate preparation as a complete operation:

~~~text
DU delta reconstruction
+ span lookup
+ repair-window construction
+ repair tokenization
+ splice
= candidate_total_prep
~~~

Compare that separately with:

~~~text
oracle full native tokenize(prefix || delta)
~~~

Do not mix model inference into this benchmark.

## Acceptance gate

Reuse the existing 144 adversarial append cases across:

~~~text
Qwen3.5-9B
Mistral-Nemo
~~~

Required for every case:

~~~text
du_delta_reconstruction_exact=true
native_snapshot_spans_exact=true
ids_exact=true
decode_exact=true

candidate_used_full_window=false
candidate_used_full_prefix_detokenize=false
oracle_used_for_verification_only=true
~~~

## Scaling gate

After 144/144:

~~~text
10 KB snapshot
64 KB snapshot tokenizer-only
1 MiB conversation-like snapshot tokenizer-only
~~~

Do not increase model inference context merely for this structural gate.

## Claim boundary

Allowed after acceptance:

~~~text
Nereids can preserve native model token spans in a snapshot, recover appended
bytes from DU structural state, repair only the mutable native-token suffix,
and reproduce full native-tokenizer IDs exactly for the tested cases.
~~~

Not yet allowed:

~~~text
native tokenizer execution is entirely eliminated
repair <= 2 tokens universally
Proteus P0 complete
KV reuse proven
infinite model context
~~~

## Proteus handoff

V2 becomes the primitive for Proteus P0:

~~~text
snapshot(prefix)
+ DU delta
-> V2 incremental native IDs
==
native_tokenize(prefix || delta)
~~~

Proteus P0 begins only after this V2 invariant is sealed.
