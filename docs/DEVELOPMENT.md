# Nereids Developer Architecture

**Status:** experimental / source-available research software
**Primary invariant:** exact canonical bytes across heterogeneous token/structural views
**Current reference implementation:** Rust
**License:** LNCL 1.2

This document explains the current Nereids code architecture, the role of DU3,
the target-vocabulary contract, the Mistral Nemo / Tekken-v3 adapter evidence,
and the boundary between exact byte interoperability and canonical tokenizer
equivalence.

---

## 1. What Nereids is

Nereids is not a tokenizer replacement and does not assume token IDs are
portable between model families.

Its core contract is:

```text
different token IDs
different token boundaries
same exact canonical bytes
```

The system separates two questions that are often conflated:

1. **Can one representation be converted into valid tokens from another
   vocabulary while preserving the exact byte stream?**
2. **Does that conversion reproduce the canonical segmentation chosen by the
   target tokenizer?**

Nereids Phase 3 already answers the first question for DU3 → target vocabulary.

The second question is measured separately with canonical tokenizer oracles and
the SpanLedger.

---

## 2. Layers

```text
raw bytes
   │
   ▼
DU3 structural codec
   │
   ├── merged.dict
   ├── local_u16
   └── gmap24 / gmap32
   │
   ▼
global DU GID stream
   │
   ├─────────────────────────────┐
   │                             │
   ▼                             ▼
Nereids Phase 3             SpanLedger
target projection           byte-span analysis
   │                             │
   ▼                             ▼
target-vocabulary IDs       canonical target IDs
   │                             │
   └──────── exact bytes ────────┘
```

### Exact layer

The following are exact invariants:

- decoding DU lexemes to canonical bytes;
- reconstructing the raw stream from DU3;
- reconstructing raw bytes from a target-vocabulary projection;
- span positions expressed in canonical byte offsets;
- hash equality used by receipts.

### Approximate / future model-state layer

Embeddings, hidden states, KV state, or learned state projection are separate
future concerns. They are not implied by byte equality.

---

## 3. Repository components

### `src/bridge.rs`

Generic byte/token bridge primitives.

Responsibilities include:

- native external token IDs;
- exact token bytes;
- contiguous byte spans;
- validation of gaps and overlaps;
- exact reconstruction;
- alignment when target token boundaries differ from structural boundaries.

This module is tokenizer-agnostic.

### `src/du_remap.rs`

Phase 3 DU → target-vocabulary projection.

Important concepts:

#### `TargetVocab`

An ID-indexed target byte vocabulary.

The index of a vocabulary entry is the target ID emitted by the bridge.

Empty entries are useful for reserved model IDs such as Mistral special/control
IDs because empty byte pieces cannot be selected by ordinary byte matching.

#### `PrecomputedDuTargetMap`

A compact global-DU-GID → target-ID expansion map.

Conceptually:

```text
gid 0 → [target_id, target_id]
gid 1 → [target_id]
gid 2 → [target_id, target_id, target_id]
```

It is stored in CSR-style form rather than as thousands of separate vectors.

```text
ids:
[a, b, c, d, e, f, ...]

offsets:
[0, 2, 3, 6, ...]
```

For `gid = 2`:

```text
start = offsets[2]
end   = offsets[3]
expansion = ids[start:end]
```

**CSR** here means *Compressed Sparse Row style storage*: one flat payload plus
an offset index.

It must not be confused with DU's local→global map.

### DU `gmap24` / `gmap32`

DU mapping:

```text
local DU ID → global DU GID
```

Nereids target mapping:

```text
global DU GID → [target-vocabulary ID, ...]
```

These are different mappings.

### `src/lib.rs` SpanLedger path

The SpanLedger aligns a supplied target token stream and DU stream by canonical
byte position.

It measures, among other things:

- target tokens exactly aligned to DU boundaries;
- target tokens crossing/cutting DU spans;
- target tokens starting inside a DU span;
- target tokens ending inside a DU span;
- maximum number of DU positions covered by one target token;
- exact reconstruction of both views.

This is the correct instrument for comparing DU structure with **canonical**
target-tokenizer output.

### `src/bin/nereids-du-remap-bench.rs`

Benchmark/receipt harness for the precomputed DU → target-vocabulary mapping.

The production-style hot path consumes the precomputed map and expands DU GIDs.
It does not need to invoke the target tokenizer for every occurrence.

---

## 4. Target vocabulary contract

A target adapter supplies an ID-indexed byte vocabulary.

Current `model.bin` format:

```text
u32 little-endian entry_count

repeat entry_count times:
    u32 little-endian byte_length
    byte[byte_length]
```

Invariant:

```text
model.bin index == emitted target ID
```

If the target vocabulary contains all 256 singleton byte values, every
canonical byte stream is representable exactly even when larger vocabulary
pieces do not align with DU boundaries.

This establishes **representability**, not canonical tokenizer segmentation.

---

## 5. Mistral Nemo / Tekken-v3 adapter

Reference vendor implementation:

```text
mistral-common 1.11.7
Tekken v3
```

Observed contract:

```text
native vocabulary slots     131,072
special/control IDs          0..999
mergeable byte-piece IDs     1000..131071
mergeable byte pieces        130,072
```

The exported Nereids target vocabulary keeps the native model ID namespace.

```text
0..999        zero-length reserved placeholders
1000..131071  Tekken mergeable byte pieces
```

This ensures ordinary byte matching cannot accidentally synthesize a model
special/control token.

Model-specific special tokens belong to adapter/model metadata and prompt
construction, not to the canonical DU byte dictionary.

---

## 6. Mistral target-vocabulary exporter

`tools/mistral/export_tekken_vocab.py`

Purpose:

- load the official vendor tokenizer definition through `mistral-common`;
- export an ID-indexed Nereids `model.bin`;
- retain Mistral native IDs;
- verify every exported mergeable piece against the vendor tokenizer;
- emit provenance metadata.

Python is intentionally an **oracle/export tool**, not the intended Nereids
runtime.

The runtime bridge remains Rust.

---

## 7. Phase 3 Mistral exact-byte result

Corpus:

```text
enwik8
100,000,000 bytes
SHA-256:
2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8
```

Real DU3-v1g stream:

```text
DU dictionary entries            426,711
DU stream positions           39,193,673
```

Phase 3 projection into the Mistral Nemo / Tekken-v3 native vocabulary:

```text
emitted target-vocabulary IDs 47,229,846
unmappable DU entries                  0
exact dictionary decode             true
exact final byte/hash match          true
```

Performance observed in that run:

```text
map build        2.362276274 s
warm remap       1.428048898 s
warm throughput  ~70.03 MB/s
peak RSS         49,924 KiB
```

This proves:

```text
real DU3
→ Rust Nereids target projection
→ valid IDs in the Mistral Nemo native vocabulary namespace
→ exact original 100 MB byte stream
```

It does **not** claim those 47,229,846 IDs are the canonical Tekken
segmentation.

---

## 8. Dictionary-level map accounting

Current map:

```text
map_target_ids = 1,370,789
```

ID payload:

```text
1,370,789 × u32(4 B)
= 5,483,156 B
```

Offset index:

```text
(426,711 + 1) × u64(8 B)
= 3,413,696 B
```

Total:

```text
8,896,852 B
```

Per unique DU dictionary entry:

```text
target-ID payload  12.8498 B
offset index        8.0000 B
total              20.8498 B
```

`map_target_ids` is the number of IDs stored **once in the dictionary-level
map**. It is not the emitted stream length.

The extra offset is the final CSR sentinel.

---

## 9. DU dictionary shape

For the tested DU3-v1g enwik8 dictionary:

```text
unique entries        426,711
decoded lexeme bytes  3,369,031
average                 7.895 B
p50                       7 B
p90                      12 B
p99                      21 B
max                     182 B
```

Examples include ordinary pieces such as:

```text
"<"
"mediawiki"
"xmlns"
"http"
"export"
```

and longer corpus-derived structures, repeated spaces, encoded Unicode forms,
identifiers, and other structural lexemes.

Percentiles are over **unique dictionary entries**, not stream occurrences.

---

## 10. Tekken dictionary shape

For the exported Nemo / Tekken-v3 mergeable byte vocabulary:

```text
non-empty entries  130,072
byte payload        878,258 B
average               6.752 B
p50                       6 B
p90                      11 B
p99                      16 B
max                      76 B
```

These are also unique-entry-weighted dictionary statistics.

Dictionary shape does not predict stream length by itself. Frequency matters.

---

## 11. Canonical Tekken oracle

`tools/mistral/export_tekken_stream.py`

This tool invokes the official Tekken implementation to create:

```text
model.bin
tokens.u32
raw-prefix.bin
metadata.json
```

`tokens.u32` contains **canonical native Mistral IDs**.

This is an oracle/test artifact. The intended production bridge must not need to
call Python or the target tokenizer for every occurrence.

---

## 12. Canonical Tekken vs DU3 — full 100 MB

Official canonical Mistral Nemo / Tekken-v3 over the same exact enwik8:

```text
canonical Tekken IDs   27,290,089
DU positions           39,193,673

bytes / Tekken ID        3.664334
bytes / DU position      2.551432
```

Ratios:

```text
Tekken IDs / DU positions = 0.696288

canonical Tekken uses
30.3712% fewer positions than DU3

DU3 uses
43.6187% more positions than canonical Tekken
```

This supersedes any sequence-density conclusion inferred from the earlier
greedy target-vocabulary projection.

The greedy projection and canonical tokenizer serve different purposes:

```text
greedy target-vocabulary projection
    proves exact target-vocabulary representability

canonical Tekken stream
    describes the actual tokenizer segmentation
```

---

## 13. Canonical boundary relationship

Full 100 MB result:

```text
canonical Tekken tokens                 27,290,089
exactly on DU boundaries                18,485,903
cross/cut DU boundaries                  8,804,186
fraction exact-boundary                    67.7385%

start inside a DU lexeme                  5,416,233
end inside a DU lexeme                    5,416,233

maximum DU positions covered
by one canonical Tekken token                    76

both byte reconstructions                       exact
```

Therefore every relationship occurs in principle:

```text
1 DU → 1 target token
1 DU → N target tokens
N DU → 1 target token
N DU → M target tokens
```

The current per-GID CSR map directly handles the first two cases.

Canonical runtime equivalence requires a boundary-aware composition path for
the latter cases.

A future optimized implementation can combine:

```text
cached per-GID expansion
        +
boundary-aware multi-GID composition
        +
cache of recurring multi-GID patterns
```

---

## 14. Same-corpus tokenizer context

Previously measured canonical `cl100k_base` over the same exact 100 MB enwik8:

```text
cl100k_base IDs      25,793,085
Tekken-v3 IDs        27,290,089
```

On this corpus, Tekken emits 1,497,004 more IDs than cl100k_base, about 5.8%
more relative to the cl100k count.

This is only a tokenizer-density observation.

It does not identify cl100k with Qwen or any other model family.

---

## 15. Reverse direction and round trips

The exact byte layer already establishes this composition:

```text
canonical Tekken IDs
→ canonical bytes
→ DU3 representation
→ canonical bytes
```

because the canonical Tekken oracle and the DU3 stream independently reconstruct
the same raw byte sequence exactly.

What has **not** yet been demonstrated is a direct identity-preserving token
mapping such as:

```text
canonical Tekken IDs
→ DU GIDs
→ canonical Tekken IDs
```

with equality of the token-ID sequence without passing through canonical bytes
and target tokenizer semantics.

That is a stronger tokenizer-equivalence property and should be tested
separately.

Likewise, deterministic equality of a freshly re-encoded DU3 structural stream
against a previous DU3 run is a codec/reproducibility question, not implied by
byte equality alone.

---

## 16. Model inference boundary

A model consumes token IDs, not abstract byte equality.

Therefore:

```text
same decoded bytes
```

does not imply:

```text
same model computation
```

if token segmentation/IDs differ.

For direct native inference the required path is:

```text
DU3
→ Nereids canonical target-ID adapter
→ native Mistral IDs
→ model runtime
```

The current Phase 3 greedy projection proves native-vocabulary
representability but not canonical Tekken-ID equality.

### LM Studio observation

An OpenAI-compatible `/v1/completions` request was tested with a JSON array of
17 canonical Nemo IDs.

The server reported:

```text
prompt_tokens = 88
```

rather than 17.

Therefore this request did **not** demonstrate a raw-token-ID inference path.
The array was accepted syntactically but was not consumed as the exact supplied
17-token native prompt.

Direct-ID inference must use a runtime/API that explicitly exposes raw native
token input, or a lower-level backend integration.

---

## 17. Model-specific special/control IDs

Special/control tokens represent model protocol rather than ordinary canonical
text bytes.

Examples include:

- BOS/EOS;
- message/role boundaries;
- tool-call delimiters;
- fill-in-the-middle markers;
- modality-specific controls, depending on model/tokenizer version.

They should remain outside ordinary byte matching.

Future adapter metadata can define them explicitly, for example:

```text
family                 mistral
tokenizer              tekken
version                v3
native_vocab_size      131072
native_byte_id_offset  1000
special_id_range       0..999
```

A future DU4 or adapter-specific extension may preserve useful model-control
symbols as a separate model dictionary without changing canonical DU byte
semantics.

---

## 18. Intended target metadata interface

The current experiments pass some provenance/claim information through
individual files and scripts.

The maintained interface should converge toward a metadata contract such as:

```text
--target-meta target.json
```

Example conceptual schema:

```json
{
  "schema": "nereids-target-v1",
  "family": "mistral",
  "model_family": "Mistral-Nemo",
  "tokenizer": "tekken",
  "tokenizer_version": "v3",
  "native_vocab_size": 131072,
  "native_byte_id_offset": 1000,
  "special_id_range": [0, 999],
  "model_bin_sha256": "...",
  "vendor_oracle": "mistral-common 1.11.7",
  "claim_boundary": "byte-exact target-vocabulary projection"
}
```

The Rust bridge should remain generic; Mistral, Qwen, cl100k-style targets, and
future adapters instantiate this contract rather than requiring duplicated
model-specific bridge implementations.

---

## 19. Proposed future canonical runtime abstraction

A future ranked-BPE adapter can remain generic.

Working name:

```text
RankedBpeTarget
```

Conceptual responsibilities:

```text
native ID offset
mergeable byte pieces
merge ranks
pretokenization pattern
special/control namespace
canonical boundary composition
```

This should be implemented only when required by the canonical-ID runtime path.

It should not replace the generic exact-byte `TargetVocab` / DU-remap layer.

---

## 20. Verification lanes

Keep claims separated.

### Lane A — exact byte projection

```text
DU3 → target vocabulary → bytes
```

Gate:

```text
SHA(raw) == SHA(DU reconstructed) == SHA(target reconstructed)
```

### Lane B — canonical tokenizer equality

```text
official tokenizer(raw) == Nereids canonical target IDs
```

Gate:

```text
same ID sequence
```

### Lane C — boundary analysis

```text
official target spans ↔ DU spans
```

Measures overlap/crossing/composition requirements.

### Lane D — direct inference

```text
Nereids native IDs → model runtime
```

Requires an API/backend that actually accepts raw token IDs.

### Lane E — performance

Measure separately:

- target-map build;
- warm DU remap;
- canonical composition;
- tokenizer baseline;
- end-to-end DU3 + Nereids;
- peak RSS;
- physical artifact size.

Do not derive tokenizer speedup claims by comparing mismatched starting points.

---

## 21. Current receipts

### Phase 3 Mistral exact-byte projection

```text
100 MB enwik8
real DU3-v1g
0 unmappable
exact target-byte reconstruction
```

Receipt SHA-256:

```text
2fbda5469094612aa0b2775e6f3999a5176b7c64b5563f27418de65d82da3332
```

### Canonical Tekken stream metadata

```text
5e77f7fa420e595981fcf3469beba8d16b172bb1d3e6c8925eb0175776d9cc2f
```

### Canonical Tekken `tokens.u32`

```text
8a759d1cfa776ec5b8beaaa011c30cd11aefb492f429debd9074c5a9d782d0bd
```

### Full 100 MB canonical SpanLedger

```text
2d8764f85e363b4b2cb6edbc1df23b8f3b41fff851d6efa7998c87c04fa8f874
```

---

## 22. Developer workflow

Core gate:

```bash
cargo fmt --check
cargo test
cargo build --release
```

Generate Rust API documentation:

```bash
cargo doc --no-deps
```

Include private implementation items while developing:

```bash
cargo doc --no-deps --document-private-items
```

Open locally when desktop integration is available:

```bash
cargo doc --no-deps --open
```

`rustdoc` is the Rust equivalent of an automatically generated code/API
reference. It derives the documentation from public items and `///` / `//!`
doc comments.

For Nereids, use both:

```text
rustdoc
    machine-generated API/reference documentation

docs/*.md
    architecture, invariants, receipts, claim boundaries, workflows
```

If the documentation grows into a handbook, `mdBook` is a natural next layer
for navigation/search while keeping Markdown as the source format.

---

## 23. Near-term roadmap

1. Commit/tag the Mistral Nemo target-vocabulary exporter, receipts, and docs.
2. Keep the Phase 3 Rust exact-byte remapper generic.
3. Add maintained `target.json` metadata instead of model-specific Rust forks.
4. Implement generic canonical ranked-BPE composition in Rust only where needed.
5. Benchmark canonical 100 MB path without Python in the production hot path.
6. Expose direct native IDs to a GPU runtime that explicitly supports raw token
   input.
7. Revisit the existing Qwen PoC and instantiate the same generic target
   contract.
8. Compare matched DU3 / Tekken / Qwen / cl100k lanes without conflating
   vocabulary size, stream length, or physical storage.
9. Model-specific special/control dictionaries remain a later adapter feature.
10. Snapshots/deltas/model-session orchestration belong to Proteus.

---

## 24. Design rule

The shortest statement of the architecture is:

```text
Bytes are identity.
DU is structure.
Tokenizer IDs are views.
Model protocol is metadata.
Model state is a separate layer.
```

Nereids should stay generic at the exact-byte/view boundary. Model-specific
adapters describe target contracts; they should not fork the core bridge unless
the underlying algorithm genuinely differs.
