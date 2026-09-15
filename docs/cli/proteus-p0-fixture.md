# Proteus P0 fixture CLI

Status: P0 developer interface.

`proteus-p0-fixture` exercises persisted conversation snapshots over
Nereids Direct-ID V2.

It operates on **exact, already-rendered conversation bytes**. It does not
render model chat templates and it does not make a model's active context
window infinite.

The P0 lifecycle is:

~~~text
exact prefix bytes
    ↓
init
    ↓
snapshot sequence 0
    ↓
append exact delta bytes
    ↓
incremental Direct-ID candidate
    ↓
full-tokenization oracle verifies afterward
    ↓
snapshot sequence 1
    ↓
append again ...
~~~

## Binary

From the repository root:

~~~bash
crates/proteus-poc/target/release/proteus-p0-fixture
~~~

Build:

~~~bash
cargo build \
  --manifest-path crates/proteus-poc/Cargo.toml \
  --release \
  --bins
~~~

## Commands

### `init`

Initialize append sequence zero from an exact prefix, its DU state, and
persisted canonical native-token spans.

~~~text
proteus-p0-fixture init \
  --prefix <PREFIX> \
  --prefix-du-run <PREFIX_DU_RUN> \
  --native-spans <NATIVE_SPANS> \
  --model-id <MODEL_ID> \
  --model-provenance <MODEL_PROVENANCE> \
  --tokenizer-id <TOKENIZER_ID> \
  --tokenizer-provenance <TOKENIZER_PROVENANCE> \
  --dictionary-lineage <DICTIONARY_LINEAGE> \
  --actor <ACTOR> \
  --source <SOURCE> \
  --snapshot <SNAPSHOT>
~~~

Arguments:

| Flag | P0 meaning |
| --- | --- |
| `--prefix` | Exact rendered prefix bytes used to initialize the conversation snapshot. |
| `--prefix-du-run` | Canonical DU run reconstructing the same prefix bytes. |
| `--native-spans` | Persisted canonical native token IDs plus exact byte spans for the prefix. |
| `--model-id` | Stable model identity stored with the snapshot. |
| `--model-provenance` | Human/machine-readable provenance for the model artifact. |
| `--tokenizer-id` | Stable tokenizer identity stored with the snapshot. |
| `--tokenizer-provenance` | Provenance for the native tokenizer used to create canonical IDs/spans. |
| `--dictionary-lineage` | Stable append-only DU dictionary lineage label. |
| `--actor` | Provenance actor for snapshot creation. |
| `--source` | Provenance source for snapshot creation. |
| `--snapshot` | Output snapshot JSON path. |

The initialized snapshot uses append sequence `0`.

The native span JSONL records use the existing Nereids native-token contract:

~~~json
{"id":123,"byte_start":0,"byte_end":5,"bytes_hex":"68656c6c6f"}
~~~

Spans must exactly and contiguously cover the prefix bytes.

### `append`

Cold-load a persisted snapshot, append exact delta bytes, construct the
incremental candidate, then consult the full-tokenization oracle.

~~~text
proteus-p0-fixture append \
  --snapshot <SNAPSHOT> \
  --delta <DELTA> \
  --delta-du-run <DELTA_DU_RUN> \
  --drop-tokens <DROP_TOKENS> \
  --native-tokenizer-server <NATIVE_TOKENIZER_SERVER> \
  --actor <ACTOR> \
  --source <SOURCE> \
  --output-snapshot <OUTPUT_SNAPSHOT> \
  --receipt <RECEIPT>
~~~

Arguments:

| Flag | P0 meaning |
| --- | --- |
| `--snapshot` | Existing validated Proteus snapshot. |
| `--delta` | Exact appended rendered bytes. |
| `--delta-du-run` | DU state reconstructing exactly those appended bytes. |
| `--drop-tokens` | Fixed number of native suffix tokens replaced by repair-window tokenization. |
| `--native-tokenizer-server` | Native tokenizer endpoint in `http://host:port` form. |
| `--actor` | Provenance actor for this append. |
| `--source` | Provenance source for this append. |
| `--output-snapshot` | Persisted next snapshot. May equal `--snapshot` for atomic in-place update. |
| `--receipt` | JSON receipt for candidate/oracle/timing evidence. |

Using a distinct `--output-snapshot` preserves the previous snapshot as an
explicit rollback/checkpoint.

P0 uses a **predeclared fixed repair depth**. The oracle does not choose the
repair depth and does not construct the candidate.

The append sequence increments monotonically:

~~~text
snapshot seq 0 + delta -> seq 1
snapshot seq 1 + delta -> seq 2
snapshot seq 2 + delta -> seq 3
...
~~~

## `validate`

Deserialize and fully validate an existing persisted snapshot:

~~~text
proteus-p0-fixture validate \
  --snapshot <SNAPSHOT>
~~~

Validation includes snapshot structural invariants, transcript identity,
native spans, dictionary lineage, append sequence, and nested DU state.

Malformed state fails closed.

## Append-only DU dictionary identity

Proteus maintains one append-only canonical DU ID namespace per lineage.

~~~text
existing exact lexeme bytes
    -> reuse existing canonical ID

new exact lexeme bytes
    -> current maximum canonical ID + 1
    -> then +1 in first-seen append order

duplicate lexeme
    -> reuse the first assigned canonical ID
~~~

Canonical IDs never move and are never renumbered.

Derived indexes such as `derived_sorted_lookup`, future rengram indexes,
tries, transducers, or other lookup structures are non-authoritative views:

~~~text
exact lexeme bytes
        ↓
derived lookup structure
        ↓
immutable canonical DU ID
~~~

They may reorganize lookup metadata but never redefine canonical identity.

## Receipt semantics

An append receipt exposes exactness and oracle-role fields including:

~~~text
previous_append_sequence
append_sequence

candidate_token_count
oracle_token_count
candidate_ids_sha256_u32le
oracle_ids_sha256_u32le

candidate_constructed_before_oracle=true
oracle_used_for_candidate_construction=false
oracle_used_for_drop_selection=false
oracle_used_for_verification=true

snapshot_persisted=true
~~~

The canonical Direct-ID gate is:

~~~text
candidate native IDs == native full-tokenization oracle IDs
~~~

The oracle is consulted only after the incremental candidate exists.

## Timing fields

P0 separates incremental resume preparation from oracle work:

~~~text
snapshot_deserialize_wall_ns
snapshot_validate_wall_ns
delta_du_reconstruct_wall_ns
direct_id_candidate_prep_wall_ns
native_span_update_wall_ns
dictionary_layer_update_wall_ns
snapshot_serialize_wall_ns
proteus_resume_total_wall_ns

oracle_full_tokenize_wall_ns
~~~

`proteus_resume_total_wall_ns` excludes oracle verification.

This allows separate comparison of:

~~~text
cold full-history tokenization

vs.

cold snapshot reload
+ snapshot validation
+ DU delta reconstruction
+ bounded Direct-ID repair
+ native-span update
+ dictionary-layer append
+ next-snapshot serialization
~~~

Model inference timing is a separate benchmark lane.

## Planned benchmark lanes

The P0 benchmark plan uses conversation-like rendered byte streams at:

~~~text
4 KiB
64 KiB
1 MiB
~~~

For each size, distinguish:

1. cold full-history native tokenization;
2. warm in-memory Proteus append;
3. cold snapshot reload + Proteus append;
4. model inference, where the model context permits it.

Large archived-state tests do not imply that the active model context is
unbounded.

## Intended chat-style wrapper

A later thin UI can sit above this fixture:

~~~text
proteus chat session.json

you> first message
model> ...

[snapshot persisted]

you> next message
...
~~~

The UI is responsible for rendering role/message structures into exact model
prompt bytes. Proteus P0 begins after those exact bytes exist.
