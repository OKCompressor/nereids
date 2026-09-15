# Proteus P1 fixture

`proteus-p1-fixture` exercises the Proteus-owned PRT0 checkpoint and PJR0
journal. P0 JSON remains available through `proteus-p0-fixture`; JSON is used
by this fixture only for the one-time bootstrap bridge, tokenizer HTTP
messages, inspection output, and append receipts.

The examples below show arguments only. Build and execution are intentionally
left to the repository's supported Rust environment.

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

## Warm benchmark

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
