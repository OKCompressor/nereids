# Task: Direct-ID V1 bounded append repair

## Goal

Implement an append-only Direct-ID repair path that reuses native snapshot
IDs and retokenizes only a bounded suffix plus the new delta.

V1 remains oracle-verified.

The full tokenizer may be used to verify the candidate receipt, but must not
be used to construct the candidate IDs.

## Existing empirical receipt

Adversarial matrix:

~~~text
Qwen3.5:
  72/72 exact
  maximum observed dropped snapshot tokens: 1
  maximum observed repaired snapshot tail: 128 bytes

Mistral-Nemo:
  72/72 exact
  maximum observed dropped snapshot tokens: 2
  maximum observed repaired snapshot tail: 64 bytes
~~~

These are measured observations, not universal tokenizer bounds.

Do not hardcode 1 or 2 as a correctness theorem.

## Architecture

Add:

~~~text
src/direct_id.rs
src/bin/nereids-direct-id-repair-fixture.rs
~~~

Export `direct_id` through `src/lib.rs`.

Do not put canonical Direct-ID repair logic in `du_remap.rs`.

`du_remap.rs` remains the byte-exact, potentially noncanonical target-vocab
projection lane.

## Candidate algorithm

Inputs:

~~~text
prefix bytes
delta bytes
native prefix IDs
max_drop_tokens
native tokenize callback
native detokenize callback
~~~

For `drop_tokens = 0..max_drop_tokens`:

1. Keep `prefix_ids[..prefix_ids.len() - drop_tokens]`.
2. Detokenize only those kept snapshot IDs.
3. Verify kept bytes are an exact prefix of the original prefix bytes.
4. Compute the repair window:

~~~text
prefix[kept_byte_len..] || delta
~~~

5. Native-tokenize only that repair window.
6. Splice:

~~~text
candidate_ids =
    kept_snapshot_ids || repair_ids
~~~

7. Return the candidate/trial metadata.

The candidate path must not tokenize `prefix || delta` in full.

## Verification oracle

The fixture CLI may separately compute:

~~~text
oracle_ids = native_tokenize(prefix || delta)
~~~

for receipt verification only.

PASS:

~~~text
candidate_ids == oracle_ids
decode(candidate_ids) == prefix || delta
~~~

The receipt must clearly distinguish:

~~~text
candidate construction work
oracle verification work
~~~

## Search policy

The fixture CLI may search from 0 through a configurable maximum number of
dropped snapshot tokens.

Default:

~~~text
--max-drop-tokens 32
~~~

Return the first exact oracle-verified candidate.

If none is exact, fail closed.

No silent full-window fallback inside the candidate path.

## CLI

~~~text
nereids-direct-id-repair-fixture \
  --prefix <path> \
  --delta <path> \
  --native-tokenizer-server <url> \
  --model-name <name> \
  --max-drop-tokens 32 \
  --receipt <json>
~~~

## Required receipt fields

~~~json
{
  "schema": "nereids-direct-id-repair-fixture-v1",
  "model_name": "...",
  "prefix_bytes": 0,
  "delta_bytes": 0,
  "full_bytes": 0,

  "prefix_sha256": "...",
  "delta_sha256": "...",
  "full_sha256": "...",

  "prefix_token_count": 0,
  "oracle_token_count": 0,
  "candidate_token_count": 0,

  "minimum_exact_drop_tokens": 0,
  "snapshot_tail_bytes_repaired": 0,
  "repair_window_bytes": 0,
  "repair_token_count": 0,

  "candidate_ids_sha256_u32le": "...",
  "oracle_ids_sha256_u32le": "...",

  "ids_exact": false,
  "decode_exact": false,

  "prefix_tokenize_wall_ns": 0,
  "candidate_repair_tokenize_wall_ns": 0,
  "oracle_full_tokenize_wall_ns": 0,

  "candidate_used_full_window": false,
  "oracle_used_for_verification_only": true,

  "trials": []
}
~~~

## Invariants

The implementation must explicitly enforce:

~~~text
candidate_used_full_window == false
ids_exact == true
decode_exact == true
~~~

for a passing fixture.

## Tests

Add unit tests independent of llama-server for splice logic.

At minimum test:

~~~text
drop 0
drop 1
drop multiple
empty delta
candidate splice order
invalid kept-prefix bytes rejected
max-drop exhausted
~~~

## Acceptance

Run Qwen and Mistral against:

~~~text
natural fixture
spaces
long ASCII
UTF-8
code
JSON-like
~~~

and deltas including:

~~~text
1 byte
UTF-8
newline
64-byte word
mixed ~256-byte delta
~~~

Required:

~~~text
Qwen:   all exact
Mistral all exact

candidate_used_full_window=false
oracle_used_for_verification_only=true
~~~

## Claim boundary

Allowed:

~~~text
For the tested append cases, Nereids reused the snapshot native-ID prefix,
retokenized only a bounded suffix plus delta, and produced exactly the same
native IDs as full native tokenization.
~~~

Not allowed yet:

~~~text
drop <= 2 tokens universally
128 bytes is a universal bound
tokenizer-independent bounded repair theorem
full tokenizer can be removed from verification
Proteus P0 is complete
~~~
