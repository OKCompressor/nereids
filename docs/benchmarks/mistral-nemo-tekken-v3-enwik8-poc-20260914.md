# Nereids → Mistral Nemo / Tekken-v3 — enwik8 PoC

Status: experimental adapter receipt.

This receipt covers a real DU3-v1g enwik8 stream projected by Nereids
into the native ID namespace of Mistral Nemo / Tekken-v3.

The demonstrated invariant is exact byte reconstruction.

Canonical Tekken segmentation equivalence is a separate gate and is not
claimed by the Phase 3 greedy remap.

## Proven input

Corpus: enwik8

Raw bytes: 100,000,000

Raw SHA-256:

    2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8

DU3 merged.dict SHA-256:

    f4b2d4602cc2215e76b2156a2aa7ee8c5030908d761efd9885d753febb50b141

## Mistral target contract

Model family: Mistral Nemo

Tokenizer: Tekken-v3

mistral-common: 1.11.7

Native vocabulary slots: 131,072

Special/control slots:

    0..999

Mergeable byte-piece IDs:

    1000..131071

Mergeable entries:

    130,072

Exported model.bin:

    1,402,550 bytes

model.bin SHA-256:

    94b7ab2b1fcfac9e6960ed12bf797d398a72d3156746383d6f433319395b6a66

The first 1,000 model.bin entries are zero-length placeholders.
Ordinary byte matching therefore cannot synthesize Mistral special IDs.

## Exact Phase 3 projection

DU dictionary entries:

    426,711

DU stream positions:

    39,193,673

Emitted Mistral/Tekken-vocabulary IDs:

    47,229,846

Unmappable DU entries:

    0

Map build:

    2.362276274 s

Warm remap:

    1.428048898 s

Warm throughput:

    70.026 MB/s

Peak RSS:

    49,924 KiB

Exact dictionary decode:

    true

Raw SHA == emitted-target-byte SHA:

    true

Receipt SHA-256:

    2fbda5469094612aa0b2775e6f3999a5176b7c64b5563f27418de65d82da3332

## Sequence density

DU stream:

    100,000,000 / 39,193,673
    = 2.551432 bytes represented per DU position

Current Mistral/Tekken-v3 vocabulary projection:

    100,000,000 / 47,229,846
    = 2.117305 bytes represented per emitted target ID

Stream-length ratio:

    47,229,846 / 39,193,673
    = 1.205038 Mistral-view IDs per DU position

The current Mistral vocabulary projection contains 20.504% more sequence
positions than the DU stream.

Equivalently, the DU stream contains 17.015% fewer positions than the
current Mistral vocabulary projection.

This is a sequence-density result.

It is not yet:
- a physical compressed-size comparison;
- a canonical Tekken-token-count comparison.

## DU dictionary shape

Unique DU lexemes:

    426,711

Decoded dictionary bytes:

    3,369,031

Average decoded bytes per unique DU lexeme:

    7.895346

Percentiles:

    p50 = 7 bytes
    p90 = 12 bytes
    p99 = 21 bytes
    max = 182 bytes

Examples include ordinary lexical pieces such as:

    "<"
    "mediawiki"
    "xmlns"
    "http"
    "export"

The dictionary also contains long structured lexemes, repeated-space
lexemes, encoded Unicode forms, long identifiers, and other corpus-derived
structures.

## Tekken dictionary shape

Non-empty mergeable pieces:

    130,072

Total byte-piece payload:

    878,258 bytes

Average bytes per mergeable Tekken piece:

    6.752091

The accompanying inspect_model_bin.py tool reports p50, p90, p99 and max
for the exported Tekken byte vocabulary.

Dictionary averages are unique-entry weighted.
They are not occurrence-weighted stream averages.

## Mapping storage

DU local/global identity and target projection are different mappings.

DU gmap32:

    local DU ID
        →
    global DU GID

Nereids Phase 3 CSR map:

    global DU GID
        →
    [Mistral/Tekken target ID, ...]

CSR means Compressed Sparse Row style storage.

The map stores one flat target-ID payload plus one offset index.

Target-ID payload:

    1,370,789 u32 IDs × 4 bytes
    = 5,483,156 bytes

Offset index:

    426,712 u64 offsets × 8 bytes
    = 3,413,696 bytes

Total current map:

    8,896,852 bytes

Per DU dictionary entry:

    target-ID payload = 12.8498 B
    offset index      =  8.0000 B
    total map         = 20.8498 B

The extra offset is the final CSR sentinel.

map_target_ids is the dictionary-level cached expansion payload.
It is not the emitted target stream length.

## Representability invariant

DU lexemes decode to canonical bytes.

A target vocabulary containing all 256 singleton byte values can
represent every canonical DU byte stream exactly.

Therefore target segmentation may:

    1 DU → 1 target token
    1 DU → N target tokens
    N DU → 1 target token
    N DU → M target tokens

The first two cases can be represented directly by the current per-GID
CSR expansion.

The latter cases require reasoning across DU boundaries for canonical
tokenizer equivalence.

They do not prevent exact byte representation.

## Canonical Tekken boundary gate

Nereids already contains a Rust span-ledger path that aligns a target
token stream and a DU stream by canonical byte positions.

For a canonical Tekken stream it can measure:

    target tokens exactly aligned to DU boundaries
    target tokens crossing or cutting DU spans
    target tokens starting inside DU spans
    target tokens ending inside DU spans
    maximum DU positions covered by one target token

The canonical experiment is therefore:

    official Tekken(full text)
            ↓
       native tokens.u32
            ↓
       Nereids SpanLedger
          ↙         ↘
    Tekken spans    DU spans
          \         /
           raw bytes

The export_tekken_stream.py helper creates the official target tokens.u32
oracle using mistral-common.

## Current claim boundary

Proven:

    real DU3-v1g enwik8
        →
    decoded canonical DU bytes
        →
    valid Mistral Nemo / Tekken-v3 native-vocabulary IDs
        →
    exact original 100,000,000 bytes

Not yet claimed:

    Phase 3 greedy IDs
        ==
    canonical Tekken(full text) IDs

Model-specific Mistral special/control IDs remain outside this plain-byte
PoC and can later be represented as adapter/model metadata without
changing canonical DU byte semantics.

## Canonical Tekken boundary experiment — 2 MB prefix

This experiment is separate from the 100 MB Phase 3 greedy
Mistral-vocabulary projection above.

The first `2,000,000` bytes of the same exact `enwik8` corpus were encoded
with the official Mistral Nemo / Tekken-v3 implementation from
`mistral-common 1.11.7`.

The `2,000,000`-byte size is an explicit test cap, not the complete
`enwik8` corpus.

| Metric | Result |
| --- | ---: |
| bytes tested | 2,000,000 |
| canonical Tekken IDs | 549,688 |
| DU positions covering prefix | 781,080 |
| bytes / canonical Tekken ID | 3.638428 |
| bytes / DU position | 2.560557 |
| canonical Tekken IDs / DU position | 0.703754 |
| target tokens exactly on DU boundaries | 371,019 |
| target tokens crossing/cutting DU boundaries | 178,669 |
| target tokens starting inside a DU lexeme | 109,365 |
| target tokens ending inside a DU lexeme | 109,365 |
| fraction exactly boundary-aligned | 67.4963% |
| maximum DU positions covered by one target token | 16 |
| exact target reconstruction | true |
| exact DU reconstruction | true |

On this 2 MB prefix, canonical Tekken uses `29.6246%` fewer sequence
positions than DU3.

Expressed in the opposite direction, the DU3 representation contains
`42.0952%` more positions than canonical Tekken on this same prefix.

These figures are sequence-position counts, not physical serialized-size
measurements.

They must not be confused with the separate 100 MB Phase 3 greedy
Mistral-vocabulary projection:

    DU3 stream positions:               39,193,673
    greedy Mistral-vocabulary IDs:      47,229,846

That 100 MB projection proves exact representability in the Mistral native
vocabulary namespace. It does not represent canonical Tekken segmentation.

The 2 MB experiment above uses the official canonical Tekken segmentation.

### Boundary relationship

Canonical Tekken and DU3 do not require matching token boundaries.

Observed on the 2 MB prefix:

    1 DU → 1 Tekken
    1 DU → N Tekken
    N DU → 1 Tekken
    N DU → M Tekken

A single canonical Tekken token covered as many as `16` DU positions in
the observed prefix.

This is why the final native-token adapter is a stream-composition problem,
not merely a one-GID-to-one-token lookup.

### Dictionary-shape comparison

DU3 `enwik8` dictionary:

    unique entries:       426,711
    decoded bytes:      3,369,031
    average:                7.895 B
    p50:                    7 B
    p90:                   12 B
    p99:                   21 B
    max:                  182 B

Mistral Nemo / Tekken-v3 mergeable vocabulary:

    non-empty entries:    130,072
    byte payload:         878,258 B
    average:                6.752 B
    p50:                    6 B
    p90:                   11 B
    p99:                   16 B
    max:                   76 B

These dictionary statistics are unique-entry weighted. They are not
occurrence-weighted sequence statistics.

Canonical span-ledger receipt SHA-256:

    bf3d74bdbcf8b1cf2cf1c4b9544df3af1f780e1b442023761c612ec1c69b05a4

## Adapter metadata follow-up

The current experimental span-ledger CLI accepts target/provenance claim
strings directly.

The intended maintained interface is a target metadata document, e.g.:

    --target-meta target.json

That metadata should carry tokenizer family/version, native ID namespace,
provenance, special-token policy, and claim boundaries while keeping the
Rust bridge target-agnostic.

This is a maintenance/interface follow-up, not a requirement for the
exact-byte or span-ledger results recorded here.

## Canonical Tekken result — full 100 MB enwik8

The definitive canonical-token experiment uses the complete
`100,000,000`-byte `enwik8` corpus, not the earlier 2 MB pilot prefix.

The same bytes were independently reconstructed by:

    real DU3-v1g structural stream
    official Mistral Nemo / Tekken-v3 token stream

The canonical Tekken stream was generated with `mistral-common 1.11.7`.

| Metric | Result |
| --- | ---: |
| bytes tested | 100,000,000 |
| DU positions | 39,193,673 |
| canonical Tekken IDs | 27,290,089 |
| bytes / DU position | 2.551432 |
| bytes / canonical Tekken ID | 3.664334 |
| canonical Tekken IDs / DU position | 0.696288 |
| exact-boundary Tekken tokens | 18,485,903 |
| cross/cut-boundary Tekken tokens | 8,804,186 |
| fraction exactly boundary-aligned | 67.7385% |
| tokens starting inside a DU lexeme | 5,416,233 |
| tokens ending inside a DU lexeme | 5,416,233 |
| maximum DU positions covered by one Tekken token | 76 |
| canonical Tekken reconstruction exact | true |
| DU reconstruction exact | true |

Canonical Tekken uses `30.3712%` fewer sequence positions than DU3 on
this corpus.

Expressed in the opposite direction, DU3 contains `43.6187%` more
positions than canonical Tekken.

These counts supersede the earlier 2 MB pilot for whole-enwik8 sequence
density. The 2 MB result remains useful as a small reproducible boundary
probe.

### Same-corpus tokenizer context

A previously measured canonical `cl100k_base` encoding of the same exact
100 MB `enwik8` corpus contained:

    25,793,085 IDs
    3.877008 bytes / ID

For this corpus:

    cl100k_base canonical IDs       25,793,085
    Mistral Tekken-v3 canonical IDs 27,290,089

Tekken therefore emits `1,497,004` more IDs than cl100k_base here,
approximately `5.804%` more relative to the cl100k count.

This is a tokenizer-density observation only. `cl100k_base` is not
identified here with any Qwen tokenizer or model family.

### What the boundary result means

The current per-GID CSR projection directly caches:

    1 DU → 1 target token
    1 DU → N target tokens

Canonical Tekken additionally requires cases such as:

    N DU → 1 Tekken token
    N DU → M Tekken tokens

On the full enwik8 run, `32.2615%` of canonical Tekken tokens cross or
cut a DU boundary.

A maintained native-token adapter can therefore combine:

    cached per-GID mappings
        +
    boundary-aware multi-GID composition
        +
    caching of repeated multi-GID patterns

The exact byte layer remains unchanged.

### Analysis-tool resource note

The current Python canonical-Tekken exporter and Rust SpanLedger are
verification/oracle tools, not the intended production hot path.

Observed during this 100 MB experiment:

    canonical Tekken export peak RSS: approximately 5.27 GB
    SpanLedger analysis peak RSS:     approximately 8.04 GB

Production bridge performance must be measured independently.

Canonical 100 MB Tekken artifacts:

    raw SHA-256:
    2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8

    canonical tokens.u32 SHA-256:
    8a759d1cfa776ec5b8beaaa011c30cd11aefb492f429debd9074c5a9d782d0bd

    canonical stream metadata SHA-256:
    5e77f7fa420e595981fcf3469beba8d16b172bb1d3e6c8925eb0175776d9cc2f

    canonical SpanLedger receipt SHA-256:
    2d8764f85e363b4b2cb6edbc1df23b8f3b41fff851d6efa7998c87c04fa8f874
