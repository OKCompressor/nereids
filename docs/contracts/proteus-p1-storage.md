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

### Frozen P1 byte layout

All integers are unsigned little-endian. All physical offsets and lengths are
`u64`. Hashes are raw 32-byte SHA-256 values. Length-prefixed byte strings use
an immediately preceding `u64`; fields declared as strings must be UTF-8.

The 88-byte PRT0 header is:

~~~text
offset  size  field
0       4     "PRT0"
4       2     version = 1
6       2     flags = 0
8       8     header_len = 88
16      8     checkpoint_sequence
24      8     section_count
32      8     directory_offset = 88
40      8     directory_len
48      32    checkpoint_root
80      8     reserved = 0
~~~

Each 56-byte directory descriptor is:

~~~text
kind u16 | flags u16 | reserved u32 | offset u64 | len u64 | sha256 [32]
~~~

Descriptor flag bit zero means optional; other bits are rejected. P1 requires
exactly one each of IDENTITY (1), TRANSCRIPT (2), NATIVE_IDS (3), NATIVE_SPANS
(4), DU_DICTIONARY (5), DU_STREAM (6), PROVENANCE (7), and STATE (8). Kind
`0x8001` is reserved for an optional SIGNATURE section. The P1 encoder emits
no signature and defines no signature or trust policy.

The checkpoint root is SHA-256 over the domain
`PROTEUS:PRT0:ROOT:V1\0`, version/flags, sequence, and each descriptor's
kind/flags/length/content hash in ascending section-kind order. It excludes
descriptor offsets and every path or filename.

Required checkpoint section bodies are:

~~~text
IDENTITY:
  dictionary lineage, model ID/provenance, tokenizer ID/provenance
  as five u64-length-prefixed UTF-8 strings

TRANSCRIPT:
  exact transcript bytes (no internal prefix)

NATIVE_IDS:
  count u64, then count native IDs as deliberately bounded u32 values

NATIVE_SPANS:
  count u64, then repeated:
    native ID u64, byte_start u64, byte_end u64,
    exact span bytes as u64-length-prefixed bytes

DU_DICTIONARY:
  next_id u64
  base_count u64, then repeated (ID u64, u64-length-prefixed lexeme bytes)
  delta_count u64, then repeated:
    append_sequence u64, entry_count u64, entries in allocation order

DU_STREAM:
  count u64, then repeated (canonical ID u64, u64-length-prefixed lexeme)
  (boundaries are checked and reconstructed from exact lexeme lengths)

PROVENANCE:
  creation event, update_count u64, update events
  event = append_sequence u64, unix_time_ns u128,
          u64-length-prefixed actor UTF-8, source UTF-8

STATE:
  transcript_len u64, transcript_sha256 [32], next_id u64
~~~

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

P1 freezes the footer at exactly 128 bytes:

~~~text
offset  size  field
0       4     "PJR0"
4       2     version = 1
6       2     flags = 0
8       8     footer_len = 128
16      8     META offset
24      8     META length
32      32    SHA-256(META)
64      32    record root
96      32    footer checksum
~~~

The checksum is SHA-256 over `PROTEUS:PJR0:FOOTER:V1\0` followed by footer
bytes 0 through 95. META must end exactly where its footer begins.

META begins with `PMT0`, version/flags, sequence, parent root, base checkpoint
root, result transcript length/hash, next canonical DU ID, one provenance
event, a `u64` section count, 56-byte descriptors, and the record root. The
required payload kinds are TRANSCRIPT_DELTA (1), DU_DICT_DELTA (2),
DU_STREAM_DELTA (3), NATIVE_ID_DELTA (4), and NATIVE_SPAN_DELTA (5).

The record root is SHA-256 over the domain `PROTEUS:PJR0:RECORD:V1\0`, the
logical META fields, provenance, and every descriptor's kind/flags/length/hash
in ascending section-kind order.
It excludes physical offsets and paths.

Required journal payload bodies are:

~~~text
TRANSCRIPT_DELTA: exact appended transcript bytes

DU_DICT_DELTA:
  append_sequence u64, entry_count u64,
  then newly allocated (ID u64, u64-length-prefixed lexeme) entries

DU_STREAM_DELTA:
  count u64, then canonical (ID u64, u64-length-prefixed lexeme) entries

NATIVE_ID_DELTA:
  kept_prefix_count u64, suffix_count u64, suffix IDs as u32

NATIVE_SPAN_DELTA:
  kept_prefix_count u64, repair_start_byte u64,
  then the NATIVE_SPANS count-and-entry encoding for the replacement suffix
~~~

The kept prefix is never serialized again in a journal record. Replay must
prove that ID/span kept counts and repair boundaries agree, the DU delta
reconstructs TRANSCRIPT_DELTA, dictionary additions are exactly max+1 in
first-seen order, and the resulting exact state matches META.

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

## Footer recovery semantics

P1 may contain multiple historical committed footer records physically.

Exactly one footer is logically active:

~~~text
the newest complete valid footer discovered from EOF
~~~

Example:

~~~text
record A + valid footer
record B + valid footer
record C + partial write

open:
  C is not a complete valid commit
  reverse discovery finds B
  canonical tip = B
~~~

Historical valid footers are immutable recovery points.

A partial trailing record does not advance the canonical head.

P1 therefore does not require a second mutable head slot or sidecar head file
for crash rollback.

A future P1.x physical packing format MAY compact history so that only one
footer remains physically present at EOF, but that is a storage optimization,
not a P1 logical requirement.

Such compaction must preserve canonical object identity and crash safety.

## Causality is not navigation

`parent_record_hash` proves which exact state a record extends.

It is not the required random-access mechanism for large histories.

Proteus MUST NOT require walking a linear parent chain to locate arbitrary
historical ranges.

Future P1.x navigation may use derived authenticated structures such as:

~~~text
radix indexes
tries / triespawn-derived indexes
range indexes
other immutable chunk indexes
~~~

Their nodes may be hash-authenticated.

The specific tree representation is not frozen by P1.

The invariants are:

~~~text
causal hashes establish ancestry

indexes locate chunks/state

the current root identifies the accepted view
~~~

Derived navigation structures never renumber canonical DU IDs or redefine
logical object identity.

## Large archive boundary

Proteus storage scale and model active-context scale are separate.

For example, a Proteus session may eventually contain:

~~~text
1 GiB exact conversation archive
~~~

without claiming that a model consumes a 1 GiB active context.

Proteus preserves exact historical state:

~~~text
conversation bytes
native token IDs
native byte spans
DU structural state
canonical DU IDs
causal lineage
~~~

A bounded active model view may be materialized from that archive separately.

P1 does not define the policy that selects or compresses that active view.
