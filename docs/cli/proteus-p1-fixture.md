# Proteus P1 fixture

`proteus-p1-fixture` exercises the Proteus-owned PRT0 checkpoint and PJR0
journal. P0 JSON remains available through `proteus-p0-fixture`; JSON is used
by this fixture only for the one-time bootstrap bridge, tokenizer HTTP
messages, inspection output, and append receipts.

The examples below show arguments only. Build and execution are intentionally
left to the repository's supported Rust environment.

## Checkpoint from exact bytes

~~~text
proteus-p1-fixture checkpoint-from-bytes \
  --transcript-bytes transcript.bin \
  --du-run transcript.du-run \
  --native-token-jsonl transcript.native.jsonl \
  --model-id Qwen3-0.6B \
  --model-provenance sha256:MODEL_WEIGHTS_HASH \
  --tokenizer-id qwen3-native \
  --tokenizer-provenance sha256:TOKENIZER_ARTIFACT_HASH \
  --dictionary-lineage session-20260915 \
  --actor ingest-worker \
  --source exact-transcript-archive \
  --checkpoint session.prt
~~~

This is the general raw-byte Proteus ingest path. The library API is
`create_checkpoint_from_exact_bytes(path, transcript_bytes,
P1ExactIngestV1)`. Its explicit inputs are a validated Nereids
`TokenizerStream`, a reconstructing `LoadedStream` DU artifact, stable
dictionary lineage, model/tokenizer identity and creation provenance. It
validates exact bytes, hash, native IDs and byte spans, DU reconstruction and
canonical base associations before using create-new PRT0 semantics. No text
decoding, normalization, provider invocation or subprocess occurs in library
code.

Proteus does not currently generate native tokenization or a DU run. The CLI
therefore requires `--native-token-jsonl` and `--du-run`. The native JSONL has
one `{id, byte_start, byte_end, bytes_hex}` object per line. The DU run is the
existing Nereids artifact directory containing `merged.dict`, `local_u16/`,
and `gmap24/` or `gmap32/`. Both must reconstruct `--transcript-bytes`
exactly. The four identity flags and the lineage/provenance flags are required;
the output is never allowed to overwrite an existing checkpoint.

## Bootstrap from P0

~~~text
proteus-p1-fixture checkpoint-from-p0 \
  --p0-snapshot session.json \
  --checkpoint session.prt
~~~

The input is fully deserialized and validated as `ProteusSnapshotV0` before
the immutable checkpoint is created. Checkpoint creation uses create-new
semantics and will not overwrite an existing file. Normal P1 open and append
never consult this JSON file.

## Inspect

~~~text
proteus-p1-fixture inspect \
  --checkpoint session.prt \
  --journal session.pjr
~~~

The result includes checkpoint/current sequence and roots, exact transcript
identity, dictionary lineage/next ID, state counts, and the committed journal
length. A missing journal is treated as an empty overlay whose head is the
checkpoint root.

## Append

~~~text
proteus-p1-fixture append \
  --checkpoint session.prt \
  --journal session.pjr \
  --expected-head HEX_SHA256 \
  --delta delta.bin \
  --delta-du-run delta.du.jsonl \
  --drop-tokens 8 \
  --native-tokenizer-server http://127.0.0.1:8080 \
  --actor fixture \
  --source exact-rendered-delta \
  --receipt append-receipt.json
~~~

`--expected-head` is the checkpoint root for the first append and the prior
record root thereafter. A stale head returns an error before candidate work
and appends zero bytes. On Unix, an exclusive advisory journal lock covers
disk-head discovery through footer sync so cooperating P1 writers cannot race
the compare-and-set. The fixed repair depth selects the same Nereids V2
Direct-ID candidate path as P0. The repair-window tokenizer constructs the
candidate; the full-tokenization oracle is called only after the candidate,
next state, journal payloads, META, record root, and footer are complete in
memory. Oracle disagreement appends zero bytes.

The JSON receipt includes roots/sequence, candidate-oracle parity, exact
transcript identity, dictionary allocation counts, bytes read/appended, the
zero historical-rewrite counter, and stage timings. The reported
`proteus_p1_append_total_wall_ns` is the sum of non-oracle append stages;
`oracle_full_tokenize_wall_ns` is separate.

## Fixed-delta depth microstress

~~~text
proteus-p1-fixture warm-bench \
  --checkpoint session.prt \
  --journal session.pjr \
  --expected-head HEX_SHA256 \
  --delta delta.bin \
  --delta-du-run delta.du.jsonl \
  --drop-tokens 8 \
  --native-tokenizer-server http://127.0.0.1:8080 \
  --iterations 100 \
  --actor fixture \
  --source warm-compute \
  --receipt warm-receipt.json
~~~

This lane opens and reconstructs the checkpoint plus accepted journal once,
checks `--expected-head` once, and then sheds all storage paths and handles.
Each iteration appends the same exact delta to the evolving in-memory state,
uses the canonical max+1 DU dictionary rules and bounded Direct-ID candidate
path, derives the next logical record head in memory, and only then invokes the
full-tokenization oracle. It does not write a journal record, META, or footer.
The journal is compared before and after the loop and must remain byte-for-byte
unchanged.

`initial_load_wall_ns` covers the one-time PRT0/PJR0 reconstruction and head
check. Each independent `warm_compute_total_wall_ns` interval starts on entry
to that iteration's in-memory advance and ends after the evolved snapshot and
logical record head are complete. It includes validation, candidate building,
native span and dictionary/DU updates, transcript identity, and logical record
encoding. The interval is frozen before `oracle_full_tokenize_wall_ns` begins;
it includes no checkpoint, journal, receipt, or delta-file I/O.

This is a controlled fixed-delta turn-depth microstress, not a diverse
conversation replay.

## Manifest-driven sequential replay

~~~text
proteus-p1-fixture warm-replay \
  --checkpoint session.prt \
  --journal session.pjr \
  --expected-head HEX_SHA256 \
  --manifest turns.json \
  --drop-tokens 8 \
  --native-tokenizer-server http://127.0.0.1:8080 \
  --oracle-every 1 \
  --actor replay-worker \
  --receipt replay-receipt.json
~~~

`turns.json` is strict JSON:

~~~json
{
  "schema": "proteus-p1-diverse-replay-manifest-v1",
  "turns": [
    {
      "delta_file": "turns/0001.bin",
      "delta_du_run": "turns/0001.du-run",
      "source": "user-turn-0001"
    },
    {
      "delta_file": "turns/0002.bin",
      "delta_du_run": "turns/0002.du-run",
      "source": "assistant-turn-0002",
      "force_oracle": true
    }
  ]
}
~~~

Relative paths resolve against the manifest directory. Empty turns, empty
source labels, unknown fields and DU/byte mismatches are rejected before
replay. Repeated exact delta payloads are valid: each occurrence is a distinct
logical event with its own sequence, parent/head, transcript position and
provenance. Existing canonical DU lexemes reuse their existing canonical IDs
normally. Workload diversity is a dataset property, not a replay invariant.
All manifest and delta/DU file reads are complete before any
`warm_compute_total_wall_ns` interval.

The checkpoint/journal is loaded once and one detached session evolves in
memory without a whole-state clone per turn. For every turn the candidate,
native spans, canonical DU layer, exact transcript identity and logical head
are fully constructed before any full native oracle call. The journal remains
unchanged. Replay provenance is deterministic: append sequence plus the
explicit `--actor` and per-turn `source`, with `unix_time_ns` fixed to zero.

Oracle verification is a policy:

- `--oracle-every N` defaults to `1`. `1` checks every turn, values greater
  than `1` check turn numbers divisible by `N`, and `0` disables scheduled
  periodic checks.
- A manifest turn with `"force_oracle": true` is always checked.
- The final turn is checked by default. `--no-final-oracle` disables only this
  automatic final check; a scheduled or forced final turn is still checked.

Thus `--oracle-every 0 --no-final-oracle` with no forced turns makes no full-
tokenization oracle calls. The bounded repair-window tokenizer remains part of
candidate construction.

`--oracle-every 1` is the certification lane. Periodic, forced and final-only
checks are the scale/performance lane. An ID mismatch is retained as
`first_mismatch` while the detached performance replay continues; an oracle
transport/protocol failure still fails the command.

The receipt records `total_turns`, `oracle_calls`, the exact set and count of
turns whose oracle comparison matched in `oracle_verified_turns`,
`oracle_every`, final-request/final-verified state, forced turns, and the first
mismatch. Every turn records the candidate native-ID SHA-256 (little-endian
`u32` encoding), transcript SHA-256, logical head and whether it was oracle
checked. The boolean
`ALL_TURNS_EXACT: true` is emitted only when every turn was actually checked
and matched. Sampled, periodic, final-only and no-oracle runs do not emit that
field merely because all sampled checks passed. A later complete offline
verifier may certify an immutable replay receipt, but P1 does not implement
that verifier here.

P1 stores each logical delta event independently. It does not physically
deduplicate repeated payloads and has no REF records. Content-addressed
physical deduplication/reference packing remains compatible with a future
container or P1.x because logical event identity is independent of physical
storage.

## Full audit

~~~text
proteus-p1-fixture full-audit \
  --checkpoint session.prt \
  --journal session.pjr
~~~

This explicit cold path walks the complete causal lineage, verifies every
footer, META, section hash, parent/base root, result identity, native span,
DU stream, dictionary layer, provenance event, and reconstructed snapshot.
It is not included in hot append timing.

## Fold

~~~text
proteus-p1-fixture fold \
  --checkpoint session.prt \
  --journal session.pjr \
  --output-checkpoint session-folded.prt
~~~

Fold writes a new create-new PRT0 checkpoint at the accepted journal sequence.
It preserves the exact transcript, native IDs/spans, DU stream and canonical
IDs, dictionary layers/next ID, lineage, model/tokenizer identity, and
provenance. It does not modify or delete the old checkpoint or journal. A new
empty journal can begin with the folded checkpoint root as its expected head.

## Commit and recovery behavior

Every record is written as payload sections, META, then one 128-byte footer.
The footer write and sync are the commit point. Open searches backward from
EOF in bounded blocks and accepts candidate magic only after the footer
checksum, exact META adjacency/bounds/hash, section ranges, and logical record
root validate. A partial trailing payload, META, or footer therefore leaves
the latest earlier complete footer as the canonical tip.
