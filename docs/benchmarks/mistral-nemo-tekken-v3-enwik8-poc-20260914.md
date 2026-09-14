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

## Native-ID inference probe

A direct native-token inference probe was performed against
`Mistral-Nemo-Instruct-2407-Q3_K_L.gguf`.

The purpose of this probe is not yet to benchmark model quality or GPU
performance. It verifies whether a model runtime consumes supplied native
Mistral token IDs directly or silently retokenizes them.

### Plain canonical Tekken prompt

Canonical text:

    DU is purrfect, ma kitty. Nereids → Mistral.

Official Tekken-v3 native IDs:

    [33674, 1395, 1274, 2370, 6567, 1044, 1914, 1409,
     43115, 1046, 1464, 1441, 4807, 8464, 42301, 2784, 1046]

Native ID count:

    17

The IDs decode exactly to the original prompt bytes.

### LM Studio OpenAI-compatible probe

The same 17 numeric IDs were supplied to:

    POST /v1/completions

LM Studio accepted the JSON request, but reported:

    prompt_tokens = 88

Therefore this endpoint did not preserve the supplied 17-token native-ID
stream as direct model input.

For this experiment, LM Studio's OpenAI-compatible endpoint is treated as a
retokenizing/text-oriented path rather than a raw-native-ID path.

### llama.cpp native `/completion` probe

The same 17 numeric native IDs were supplied directly to llama.cpp:

    POST /completion

Observed:

    supplied native IDs: 17
    tokens_evaluated:    17
    prompt_n:            17
    tokens_cached:       17

llama.cpp reconstructed the prompt as:

    DU is purrfect, ma kitty. Nereids → Mistral.

The model generated native ID:

    [2]

and stopped immediately with:

    stop_type = "eos"

For Mistral Nemo / Tekken-v3:

    EOS native ID = 2

This establishes that llama.cpp consumed the supplied numeric IDs directly,
without retokenizing the prompt.

The immediate EOS is not considered a failure of native-ID injection. The
tested model is an instruct model and the first probe intentionally omitted
the model's chat/instruction control structure.

### Official Mistral instruct serialization

The same tokenizer family was then exercised through the official
`mistral-common 1.11.7` chat serializer.

User message:

    Reply with exactly this phrase and nothing else: Purrfect bridge.

Official serialized native IDs:

    [1, 3, 53156, 1454, 10693, 1593, 24528, 1321, 7545,
     2849, 1058, 1390, 2370, 6567, 18314, 1046, 4]

Serialized ID count:

    17

The first and last portions are:

    first:
    [1, 3, 53156, 1454, 10693, 1593, 24528, 1321, 7545, 2849, 1058, 1390]

    last:
    [1593, 24528, 1321, 7545, 2849, 1058, 1390, 2370, 6567, 18314, 1046, 4]

These IDs demonstrate the separation between ordinary byte-piece IDs and
model-protocol IDs.

The Mistral Nemo / Tekken-v3 native namespace reserves:

    0..999       model special/control IDs
    1000..131071 mergeable byte-piece IDs

Known in this experiment:

    ID 1 = BOS
    ID 2 = EOS

IDs `3` and `4` are serializer-inserted model-control IDs from the reserved
special namespace. They are not produced by ordinary DU byte matching.

This is why Nereids keeps model protocol separate from canonical DU bytes:

    canonical DU bytes
        +
    model-specific control metadata
        =
    valid native instruct stream

### Current inference claim boundary

Proven:

    explicit native numeric IDs
        →
    llama.cpp `/completion`
        →
    exactly the same number of prompt tokens evaluated

Specifically:

    17 supplied IDs
        →
    17 tokens evaluated

Not yet claimed:

    DU3
        →
    canonical Tekken-v3 instruct IDs entirely in Rust
        →
    identical model output to the official Mistral tokenizer/serializer path

The immediate next inference gate is:

    real DU3
        →
    Nereids Rust native-ID projection
        →
    Mistral model-control IDs
        →
    llama.cpp
        →
    GPU inference

A later canonical gate replaces the current greedy target-vocabulary
projection with a Rust implementation that reproduces the official
Tekken-v3 canonical ID stream.

## Successful direct native-ID instruct inference

The direct native-ID path was then tested using the official
Mistral instruct serialization.

User instruction:

    Reply with exactly this phrase and nothing else: Purrfect bridge.

Official serialized native prompt IDs:

    [1, 3, 53156, 1454, 10693, 1593, 24528, 1321, 7545,
     2849, 1058, 1390, 2370, 6567, 18314, 1046, 4]

Prompt ID count:

    17

These numeric IDs were supplied directly to llama.cpp `/completion`
with no text prompt and `temperature = 0`.

Runtime:

    model:
    Mistral-Nemo-Instruct-2407-Q3_K_L.gguf

    backend:
    llama.cpp Vulkan

    device:
    AMD Radeon 780M Graphics

Observed by llama.cpp:

    tokens_evaluated = 17
    prompt_n         = 17

Decoded prompt reported by the runtime:

    <s>[INST]Reply with exactly this phrase and nothing else: Purrfect bridge.[/INST]

Generated native IDs:

    [1080, 2370, 6567, 18314, 2]

Decoded model output:

    Purrfect bridge

The final generated token is:

    2 = EOS

The runtime stopped with:

    stop_type = "eos"

This is direct native-ID model execution: the number of numeric prompt IDs
supplied to the runtime exactly equals the number of prompt tokens evaluated.

No target tokenizer was invoked by the inference endpoint.

### Observed timings

This run was a functionality probe on the integrated AMD GPU, not a production
performance benchmark.

Observed:

    prompt tokens:             17
    prompt eval:               394.603 ms
    prompt throughput:         43.08 tok/s

    generated tokens:          5
    generation time:           352.356 ms
    generation throughput:     11.35 tok/s

    total measured model time: 746.96 ms

These numbers should not be compared with the Nereids DU-remap benchmark.
They measure model execution on a different hardware/runtime path.

### What is now proven

Proven independently:

    DU3
        →
    Nereids Rust target-vocabulary projection
        →
    valid native Mistral vocabulary IDs
        →
    exact original bytes

and:

    explicit native Mistral instruct IDs
        →
    llama.cpp
        →
    model execution
        →
    native output IDs

The remaining integration gate is to join those two proven paths directly:

    DU3
        →
    Nereids Rust
        →
    native Mistral IDs
        +
    model-control IDs
        →
    llama.cpp
        →
    inference

After that, the stronger canonical gate is:

    DU3
        →
    Rust canonical Tekken composition
        →
    the same native IDs as the official tokenizer
        →
    model inference

## Same-bytes inference comparison: canonical Tekken vs DU3 → Nereids

A controlled direct-ID inference comparison was performed over the exact same
582-byte prefix of the real `enwik8` corpus.

Source SHA-256:

    184256daf46cb683a1885a41ff97b3cc9b55790ea4465b37c794b480c7d23d25

Both input paths reconstruct those 582 bytes exactly.

Both use the same model:

    Mistral-Nemo-Instruct-2407-Q3_K_L.gguf

Both use the same Mistral model-control framing:

    [1, 3] + content IDs + [4]

Both were supplied directly as numeric native IDs to llama.cpp `/completion`
with:

    temperature = 0
    cache_prompt = false
    n_predict = 64

### Input path A — canonical Tekken

The 582 source bytes were encoded by the official `mistral-common 1.11.7`
Tekken-v3 tokenizer.

    content IDs:       181
    model-control IDs:   3
    total prompt IDs:   184
    exact bytes:        true

Observed model execution:

    tokens_evaluated:   184
    prompt eval:        1379.396 ms
    prompt throughput:  133.39 tok/s
    generated tokens:   64
    generation time:    5329.819 ms
    generation rate:    11.82 tok/s

### Input path B — real DU3 → Nereids Rust

The same source bytes were represented by a real DU3-v1g stream and projected
into the native Mistral Nemo vocabulary by the Rust Nereids Phase 3 remapper.

The native target IDs were read directly from the Rust `--id-output` artifact.

    DU-derived content IDs: 256
    model-control IDs:        3
    total prompt IDs:       259
    exact bytes:            true

Observed model execution:

    tokens_evaluated:       259
    prompt eval:            2193.422 ms
    prompt throughput:      118.08 tok/s
    generated tokens:       64
    generation time:        5355.579 ms
    generation rate:        11.76 tok/s

### Comparison

| Metric | Canonical Tekken | DU3 → Nereids Rust |
| --- | ---: | ---: |
| source bytes | 582 | 582 |
| content IDs | 181 | 256 |
| total prompt IDs | 184 | 259 |
| bytes / content ID | 3.2155 | 2.2734 |
| prompt eval | 1379.396 ms | 2193.422 ms |
| prompt throughput | 133.39 tok/s | 118.08 tok/s |
| generated IDs | 64 | 64 |
| generation throughput | 11.82 tok/s | 11.76 tok/s |

For this prefix, the current greedy Nereids target projection emits `41.44%`
more content IDs than canonical Tekken.

Equivalently, canonical Tekken uses `29.30%` fewer content positions than the
current greedy Nereids projection.

Prompt evaluation for the longer greedy stream took `59.01%` more wall time in
this functionality run.

Generation throughput after prompt evaluation was nearly unchanged.

### Model-output observation

The two paths contain exactly the same canonical source bytes but use different
native token segmentations.

Both model outputs interpreted the input as MediaWiki XML and began producing
a cleaned-up XML representation, but the generated text was not identical.

This demonstrates an important boundary:

    exact byte equivalence
        does not imply
    identical model computation

Canonical native-ID equivalence therefore remains relevant when exact model
behavior is required.

### End-to-end DU3 inference result

This experiment closes the first direct model-execution path:

    real DU3-v1g
        →
    Nereids Rust
        →
    valid native Mistral Nemo vocabulary IDs
        →
    Mistral model-control IDs
        →
    llama.cpp raw numeric-ID input
        →
    model execution

For the tested prefix:

    DU-derived native IDs: 256
    total supplied IDs:    259
    tokens evaluated:      259

No target tokenizer was invoked in this DU3-derived inference input path.

This is not yet canonical Tekken-equivalent inference. The current Rust
projection uses exact-byte greedy target segmentation.

The stronger next gate is:

    real DU3
        →
    Rust canonical target composition
        →
    identical native IDs to official Tekken-v3
        →
    direct model inference

## Planned Rust segmentation modes

The current Phase 3 remapper and the future canonical adapter solve different
optimization problems.

### Mode A — per-GID greedy

Current implementation:

    global DU GID
        →
    greedy target-vocabulary expansion

Matching is performed independently inside each decoded DU lexeme.

Properties:

    exact bytes
    precomputable per DU dictionary entry
    very cheap at stream time
    cannot compose target tokens across DU boundaries
    not canonical Tekken segmentation

### Mode B — stream greedy-longest

A future boundary-aware mode may match target vocabulary pieces across
consecutive DU lexemes.

Conceptually:

    DU spans
        →
    continuous canonical byte stream
        →
    longest available target pieces

This can reduce the extra target positions created by per-GID boundaries while
remaining independent of the target tokenizer implementation.

It remains a non-canonical native-token view.

### Mode C — canonical ranked-BPE

Canonical Tekken equivalence requires reproducing the target tokenizer's actual
segmentation rules rather than selecting only the longest byte piece.

A Rust canonical composer therefore needs the target contract required by the
tokenizer, including where applicable:

    byte pieces
    native IDs
    merge ranks
    pretokenization rules
    special/control namespace

The verification gate is:

    Rust canonical IDs
        ==
    official mistral-common Tekken-v3 IDs

over the same byte stream.

### Mode D — minimum-token native view

Because multiple valid native token sequences can reconstruct the same bytes,
Nereids can also experimentally search for the shortest legal target-ID
sequence.

This is a different objective from canonical BPE.

A dynamic-programming formulation over canonical byte offsets can minimize the
number of target IDs:

    dp[offset] = minimum target IDs required to reach offset

Candidate vocabulary pieces are bounded by the target vocabulary's maximum
byte-piece length.

For the current Mistral Nemo target:

    maximum mergeable piece length = 76 bytes

This mode would answer:

    what is the minimum number of valid native Mistral IDs
    capable of representing these exact bytes?

Such a stream may be shorter than canonical Tekken, but it is not necessarily
distributed like the token sequences seen during model training. Model behavior
must therefore be measured rather than assumed equivalent.

### Why keep these modes separate

The objectives are different:

    per-GID greedy
        optimize structural reuse and lookup cost

    stream greedy-longest
        aggressively remove DU-boundary fragmentation

    canonical ranked-BPE
        preserve official tokenizer behavior

    minimum-token DP
        minimize native sequence length

All four can preserve exact bytes while producing different model-state
trajectories.

## Near-context 4-lane direct-inference probe

A larger same-bytes experiment used an 8,356-byte prefix reconstructed from
the real DU3-v1g `enwik8` stream.

Source SHA-256:

    5baa7297379bfe44e0f3e7d6ae247f716b2c3b5c67610f1d5a8cfbc5fc50f5f4

All four lanes reconstruct exactly the same source bytes.

The llama.cpp server used:

    n_ctx_slot = 4096
    n_predict = 128
    temperature = 0

The longest lane reached:

    3803 prompt IDs + 128 requested generation IDs = 3931 positions

or 95.97% of the configured 4096-token slot context.

This is a near-configured-context functionality probe. It is not a claim about
the maximum context capacity of the hardware or model.

### Four segmentation lanes

| Lane | Purpose | Content IDs | Bytes / ID |
| --- | --- | ---: | ---: |
| A: per-GID greedy | current DU3 structural remap | 3800 | 2.1989 |
| B: stream-longest greedy | experimental cross-DU cost-oriented view | 2605 | 3.2077 |
| C: canonical Tekken | official tokenizer reference | 2623 | 3.1857 |
| D: exact-min DP | mathematical minimum-token oracle | 2567 | 3.2552 |

The principal experimental comparison is B versus C.

On this exact 8,356-byte prefix:

    stream-longest:   2605 content IDs
    canonical Tekken: 2623 content IDs

Therefore the stream-longest experimental view used:

    18 fewer content IDs
    0.686% fewer content IDs

than canonical Tekken on this prefix.

This is an observed result for this prefix, not a corpus-wide claim.

Lane D establishes the legal-vocabulary lower bound for this byte sequence:

    exact-min DP:     2567 content IDs
    canonical Tekken: 2623 content IDs
    difference:         56 IDs

or approximately 2.135% fewer content IDs than canonical Tekken.

Lane D is an analysis oracle, not a proposed canonical tokenizer.

### Cold direct-ID inference

Each lane was supplied directly as native numeric IDs to the same
Mistral-Nemo-Instruct-2407 model through llama.cpp.

Three cold runs were performed per lane.

Mean prompt-evaluation times:

| Lane | Prompt IDs | Mean cold prompt eval |
| --- | ---: | ---: |
| A: per-GID greedy | 3803 | 22271.722 ms |
| B: stream-longest greedy | 2608 | 14715.211 ms |
| C: canonical Tekken | 2626 | 14930.060 ms |
| D: exact-min DP | 2570 | 14542.602 ms |

Relative to canonical Tekken on this run:

    B stream-longest:
        0.686% fewer content IDs
        1.44% lower mean cold prompt-eval wall time

    D exact-min:
        2.135% fewer content IDs
        2.60% lower mean cold prompt-eval wall time

These are single-prefix functionality measurements on the AMD Radeon 780M
Vulkan backend. They are not generalized performance claims.

### Exact-repeat cache baseline

With `cache_prompt=true`, llama.cpp reused all but one prompt token:

    A: cache_n = 3802 / 3803
    B: cache_n = 2607 / 2608
    C: cache_n = 2625 / 2626
    D: cache_n = 2569 / 2570

Mean warm prompt evaluation:

    A: 99.098 ms
    B: 92.445 ms
    C: 91.317 ms
    D: 92.921 ms

At this point prompt length is no longer the dominant cost: llama.cpp is
evaluating only one prompt token.

This exact-repeat result is a cache baseline, not a Proteus delta result.
The relevant future experiment is a large cached prefix followed by a small
append or edit, measuring the retained native-ID prefix, rollback distance,
suffix evaluation, and final output against a clean recomputation.

### Qualitative output observation

The source bytes and model were identical, but generated behavior differed
across token segmentations.

The canonical Tekken lane began by extracting document content directly:

    Page Title: Anarchism

and continued with revision metadata including the revision ID, timestamp and
contributor.

The per-GID, stream-longest and exact-min lanes instead began primarily by
describing or summarizing the surrounding MediaWiki XML structure.

No explicit semantic question was supplied, so this is not a scored quality
comparison. It is evidence that exact byte identity does not imply identical
model-state trajectories or identical generations.

Representative first-run outputs are preserved separately in:

    docs/benchmarks/mistral-nemo-4lane-ctx3800-output-samples-20260914.txt

### Construction timing boundary

The experimental lane builder also recorded:

    stream-longest Python prototype: 25.000 ms
    mistral-common canonical Tekken:  1.828 ms
    exact-min Python DP oracle:       78.936 ms

The current per-GID Rust lane was consumed from an already-produced Rust
artifact and therefore has no comparable construction timing in this window.

These construction timings must not be used as the final tokenizer-performance
comparison. The principal future benchmark is:

    official Tekken / tiktoken native implementation
        versus
    Nereids stream-longest implemented natively in Rust

from equivalent input bytes and under matched measurement conditions.

### Raw receipts

Segmentation receipt:

    receipts/mistral-nemo-4lane-ctx3800-segmentation-20260914.json

Inference receipt:

    receipts/mistral-nemo-4lane-ctx3800-inference-20260914.json

Representative outputs:

    docs/benchmarks/mistral-nemo-4lane-ctx3800-output-samples-20260914.txt
