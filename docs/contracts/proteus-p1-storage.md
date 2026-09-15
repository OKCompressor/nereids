# Proteus P1 storage contract

Status: implementation contract.

Proteus P1 replaces the P0 whole-snapshot JSON hot path with a
Proteus-owned binary checkpoint + append journal format.

P0 remains the exactness baseline.

## Identity invariants

Canonical DU identity is append-only.

~~~text
existing exact DU lexeme bytes
    -> reuse immutable canonical ID

new exact DU lexeme bytes
    -> max canonical ID + 1
    -> then +1 in deterministic first-seen order

later duplicate
    -> reuse original ID
~~~

Canonical IDs are never renumbered by:

- checkpoint creation,
- journal packing,
- sorting,
- indexing,
- chunk merge/split,
- replication,
- physical relocation,
- derived rengram/transducer structures.

Logical identity and physical packing are separate concepts.

## Minimal physical layout

A session initially consists of:

~~~text
session.prt
session.pjr
~~~

`session.prt` is an immutable checkpoint.

`session.pjr` is an append-only overlay/journal.

Later physical segmentation may produce:

~~~text
session.prt
journal-000.pjr
journal-001.pjr
journal-002.pjr
~~~

Segmentation does not alter logical history.

## PRT0 checkpoint

Conceptual checkpoint state:

~~~text
magic = PRT0
version
flags

checkpoint_sequence
dictionary_lineage_hash

model_identity_hash
tokenizer_identity_hash

transcript_len
transcript_sha256

next_canonical_du_id

native_ids
native_spans

DU canonical dictionary
DU structural stream

section_directory
section_hashes

checkpoint_root_hash
~~~

The checkpoint is immutable after commit.

A later checkpoint may fold earlier journals physically, but all canonical
DU IDs and exact state identities remain unchanged.

## Checkpoint signatures

The binary format reserves a signature section.

If present, a signature covers a domain-separated checkpoint root, not the
mutable physical filename or storage location.

Conceptually:

~~~text
SIGNATURE
  algorithm
  key_id
  signed_root
  signature_bytes
~~~

Distributed trust or quorum validation is not required by P1.

## PJR0 records

Each journal append represents one causal update.

Conceptually:

~~~text
sequence
parent_record_hash
base_checkpoint_root_hash

delta_len
delta_sha256

new_du_entries
du_structural_delta

native_repair_boundary
native_id_delta
native_span_delta

result_transcript_len
result_transcript_sha256
next_canonical_du_id

record_root_hash
~~~

The first journal record after a checkpoint binds to the checkpoint root.

Later records bind to the immediately previous canonical record.

## Sectioned record model

A record is made from independently hashable sections.

Canonical initial section types:

~~~text
TRANSCRIPT_DELTA
DU_DICT_DELTA
DU_STREAM_DELTA
NATIVE_ID_DELTA
NATIVE_SPAN_DELTA
~~~

Optional non-authoritative sections may later include:

~~~text
RENGRAM_INDEX
TRANSDUCER_INDEX
TRIE_INDEX
OTHER_OKC_INDEX
SIGNATURE
~~~

Derived sections may be omitted, rebuilt, replaced, or stored elsewhere.

They never define canonical DU identity.

## Payload first, metadata at EOF

P1 uses payload-first layout.

~~~text
[payload section 0]
[payload section 1]
[payload section 2]
...
[META]
[FIXED FOOTER]
~~~

Metadata lives at the end because it describes the variable payload that
precedes it.

The loader begins at EOF, reads the fixed footer, then jumps directly to
the current metadata.

The footer uses fixed-width fields, including at least:

~~~text
magic
version
meta_offset       u64
meta_len          u64
meta_hash
record_or_view_root
footer_checksum
~~~

A 64-bit offset/length contract avoids a 4 GiB limit.

## Strict append-only commit layout

Proteus prefers retaining previous committed tails rather than truncating
historical metadata.

~~~text
payload A
META A
FOOTER A

payload B
META B
FOOTER B

payload C
META C
FOOTER C
~~~

The newest complete valid footer is the committed tip.

If writing C is interrupted before its valid footer is complete, B remains
the canonical committed state.

Normal append rewrites zero historical canonical payload bytes.

## Logical objects vs physical packing

A logical object is identified by exact content hash.

The same logical object may physically exist:

~~~text
as a loose object
inside one PJR pack
inside a merged journal segment
inside a checkpoint pack
on another peer
inside a partial replicated view
~~~

Physical location is not identity.

A small locator/index layer may map:

~~~text
content hash -> current physical location
~~~

without changing any object reference.

## Journal chunking and merge

Many small journal records may later be packed:

~~~text
journal-000.pjr
  records 1..1024

journal-001.pjr
  records 1025..2048
~~~

Packing or merging changes physical layout only.

Checkpoint folding may produce:

~~~text
checkpoint @ 0
+ journals 1..2048

        ->

checkpoint @ 2048
journal resumes @ 2049
~~~

No canonical ID translation or renumbering is permitted.

## One canonical head per logical interaction stream

Each logical interaction stream has one canonical head.

A writer prepares a record against an expected parent:

~~~text
expected_parent = H
~~~

Commit succeeds only if:

~~~text
current_head == H
~~~

If the current head has advanced, the append is stale.

The writer must obtain the missing tip/ancestry, reconstruct the new state,
and retry against the current head.

This is compare-and-set semantics.

It prevents a stale local writer from silently replacing canonical history.

## Partial views

A replica does not need every physical pack.

It may possess only a partial view such as:

~~~text
checkpoint X
journal segment A
journal segment C
derived index R
~~~

Metadata must make missing canonical dependencies detectable.

If required ancestry is absent, the view is incomplete.

The missing object or segment must be fetched or otherwise supplied before
the replica claims the complete canonical state.

## Incremental validation

Normal append validates only what is required to extend the already accepted
tip safely:

~~~text
current head
sequence
parent hash
checkpoint association

new DU allocations
repair boundary
native ID delta
native span continuity

new transcript length/hash
new section hashes
new footer
~~~

P1 does not require rescanning and reserializing all historical state on
every append.

A separate full-audit operation reconstructs and validates the complete
checkpoint + journal lineage.

Checkpoint creation requires full reconstruction/audit before the new
checkpoint becomes authoritative.

## Derived indexes

Canonical storage remains append-order truth.

Derived structures may organize the same lexemes differently:

~~~text
sorted lookup
rengrams
tries
transducers
other OKC structures
~~~

They map exact lexeme bytes to immutable canonical IDs.

They do not rewrite the dictionary.

## P1 non-claims

P1 does not yet establish:

~~~text
distributed consensus
Byzantine quorum / 51-percent validation
P2P gossip
federated authority
automatic semantic merge of concurrent interaction streams
arbitrary-order semantic turn composition
cross-model KV portability
infinite active model context
~~~

These are later-phase questions, not impossibilities.
