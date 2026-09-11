# Phase 2 exact tokenizer bridge

The Phase 2 bridge is byte-based and tokenizer-ID agnostic.

`NativeToken` stores a native integer ID, an absolute byte span, and the
exact bytes for that span. `TokenizerStream` adds tokenizer name,
provenance, raw SHA-256, raw length, and the ordered token list. A stream is
valid only when its spans are contiguous, its token bytes match the raw input,
and concatenating its token bytes reproduces the raw input exactly.

`NativeTokenLike` is the adapter boundary for externally produced records.
JSONL import/export uses records with `id`, `byte_start`, `byte_end`, and
`bytes_hex`; metadata is supplied separately so simple tokenizer tools can
remain line-oriented.

`bridge_tokenizer_streams` validates source, DU, and target streams, emits
source→DU, DU→target, and target→DU span alignments, and returns a report with
reconstruction checks, mismatch fields, provenance, and boundary/crossing
statistics. Native source and target IDs are intentionally never compared.

This layer does not provide native tokenizer execution or claim native-ID
delta equivalence. Its absolute byte spans and preserved native IDs leave the
later full-tokenize/incremental-ID test possible without changing the core
representation.
