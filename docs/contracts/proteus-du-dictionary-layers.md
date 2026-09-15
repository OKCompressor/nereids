# Proteus DU dictionary layer contract

Status: P0 exact-byte contract.

## Identity

A dictionary key is the exact DU lexeme byte sequence represented by
`DictionaryEntryV0.lexeme_bytes`. P0 performs no Unicode, whitespace,
case-folding, linguistic, or other normalization. Empty lexemes are rejected.

## One append-only namespace

`DuDictionaryLayersV0` contains one immutable base layer, zero or more delta
layers, a stable lineage label, and `next_id`. Every layer shares one global
canonical ID namespace:

- every base ID remains unchanged;
- an exact byte match in any existing layer reuses that entry's ID;
- an unseen exact lexeme is appended to the current append's newest delta
  layer;
- unseen lexemes receive consecutive IDs in first-seen stream order;
- another appearance of the same bytes reuses the first allocation;
- once assigned, a canonical ID is immutable for the life of the lineage;
- every appended message/delta creates one layer, including an empty layer
  when it introduces no lexemes;
- layer sequence numbers are consecutive and begin at one.

Incoming delta IDs are structural input metadata, not canonical assignment
authority. Exact lexeme bytes select an existing canonical ID or allocate the
next ID. A single incoming ID associated with different bytes is malformed and
rejected.

Base entries preserve the exact IDs found in the initialized prefix DU state
and are serialized in increasing ID order. Delta allocation starts at one
past the largest base ID; base gaps are not filled or repurposed. Before every
later allocation, `next_id` is exactly one past the largest assigned canonical
ID. The current delta is traversed as stored and is never sorted before ID
assignment.

Snapshot validation requires every `(DU ID, lexeme bytes)` pair in the nested
Nereids stream to resolve to the same pair in the snapshot's dictionary
lineage. Duplicate IDs, duplicate exact lexemes, out-of-order base IDs,
nonconsecutive delta IDs, bad sequence numbers, or an incorrect `next_id` fail
closed.

## Derived lookup metadata

`derived_sorted_lookup` may order copies of dictionary entries by exact lexeme
bytes to accelerate or inspect lookup. For example, canonical entries
`41 -> zebra`, `42 -> apple`, and `43 -> river` produce the sorted lookup
`apple -> 42`, `river -> 43`, `zebra -> 41`.

This lookup is derived metadata only. It does not mutate the base, delta
layers, persisted snapshots, or any canonical ID. P0 exposes no operation that
changes an assigned canonical identity. Consequently, a snapshot saved before
later appends remains directly interpretable without migration metadata.

## Determinism

Given the same base `(ID, exact bytes)` associations and the same ordered
sequence of delta lexeme bytes, the base layer, every delta layer, assigned
IDs, and canonical DU streams are identical. Wall-clock timestamps and model
metadata do not participate in dictionary allocation.
