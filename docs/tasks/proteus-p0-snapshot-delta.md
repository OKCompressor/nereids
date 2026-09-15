# Proteus P0 snapshot/delta proof

Status: implementation contract for `crates/proteus-poc`.

## Scope

Proteus P0 is an orchestration and persistence layer above Nereids Direct-ID
V2. It accepts exact conversation bytes that a caller has already rendered.
It does not render roles, messages, or model chat templates, and therefore
does not claim compatibility with every chat template.

The P0 gate for persisted prefix `P` and appended bytes `D` is:

```text
native_full_tokenize(P || D)
==
proteus_incremental_native_ids(P_snapshot, D)
```

The incremental candidate must exist in full before the full-history oracle
is called. There is no full-tokenization fallback in candidate construction.
Proteus archives exact conversation state; active model context remains
bounded by the selected model and runtime.

## Snapshot V0

`ProteusSnapshotV0` stores:

- an exact schema and version;
- a monotonic append sequence starting at zero;
- the immutable DU dictionary lineage;
- model and tokenizer identities plus their provenance;
- exact transcript byte length and SHA-256;
- a nested `DirectIdSnapshotV2`, which remains the owner of canonical native
  IDs, exact native byte spans, and reconstructing `LoadedStream` DU state;
- deterministic base/delta dictionary metadata;
- one creation provenance event and one update event for every append.

Validation is fail-closed. It reconstructs transcript bytes from DU state and
revalidates Nereids V2 invariants, redundant Proteus length/hash fields,
dictionary IDs and lexemes, sequence counts, lineage identity, and provenance.
Malformed snapshots are not candidates for repair.

## Append pipeline

For a fixed repair depth, `resume_snapshot_json` performs these operations:

1. deserialize and validate the complete prior snapshot;
2. reconstruct the supplied delta DU state and require exact equality with
   the supplied delta bytes;
3. call Nereids `construct_du_aware_repair_candidate` with a repair-window
   tokenizer callback;
4. retain the callback's exact token pieces, require that they reconstruct
   the repair window, and shift their spans from the persisted repair byte;
5. update the append-only dictionary layer and concatenate the canonicalized
   DU delta to the prior DU state;
6. construct and validate a new nested `DirectIdSnapshotV2`;
7. serialize the complete next snapshot.

No old native prefix is detokenized. The old repair boundary comes directly
from persisted native spans. A small checked `LoadedStream::concatenate` API
is the only Nereids addition required for multi-turn composition; it changes
neither loader behavior nor any ID semantics.

Only after the complete resume artifact exists may
`verify_with_full_tokenize_oracle` tokenize the full reconstructed transcript.
The API accepts no oracle IDs during candidate construction.

On an equality failure, the fixture writes a failure receipt but does not
install the candidate snapshot. Snapshot installation uses a same-directory
temporary file and atomic rename. Using a distinct output path retains the
prior snapshot as a rollback point.

## Fixture

The crate is intentionally independent of the root workspace:

```text
cargo run --manifest-path crates/proteus-poc/Cargo.toml --bin proteus-p0-fixture -- init \
  --prefix prefix.txt \
  --prefix-du-run runs/prefix-du \
  --native-spans prefix-native.jsonl \
  --model-id model-name \
  --model-provenance sha256:model-weights \
  --tokenizer-id tokenizer-name \
  --tokenizer-provenance sha256:tokenizer-config \
  --dictionary-lineage chat-2026-09-15-a \
  --actor fixture \
  --source rendered-chat-v0 \
  --snapshot snapshots/s0.json
```

```text
cargo run --manifest-path crates/proteus-poc/Cargo.toml --bin proteus-p0-fixture -- append \
  --snapshot snapshots/s0.json \
  --delta turn-1.txt \
  --delta-du-run runs/turn-1-du \
  --drop-tokens 8 \
  --native-tokenizer-server http://127.0.0.1:8080 \
  --actor fixture \
  --source rendered-turn-1 \
  --output-snapshot snapshots/s1.json \
  --receipt receipts/proteus-p0-turn-1.json
```

The llama-server `/tokenize` repair request uses `add_special=false` and
`with_pieces=true`. P0 requires the returned pieces to reconstruct the exact
repair-window bytes. The full oracle uses a later, separate `/tokenize`
request. The fixture's HTTP adapter accepts `http://host:port`; TLS and chat
rendering belong outside this proof fixture.

## Explicit non-claims

P0 does not claim infinite model context, prefix KV reuse, inference savings,
cross-model KV portability, chat-template rendering, linguistic dictionary
normalization, learned embeddings, hidden-state projection, or KV projection.

## CLI reference

The P0 fixture command-line contract is documented in [`docs/cli/proteus-p0-fixture.md`](../cli/proteus-p0-fixture.md).
