# Direct-ID Benchmark Plan

## Objective

Measure whether Nereids can feed Mistral and Qwen with the exact same
native token IDs produced by their own tokenizer, then compare latency
paths without mixing tokenization speed, prefill speed, and generation
speed.

## Fixture ladder

Run in this order:

1. tiny deterministic fixture
2. 4 KiB fixture
3. 64 KiB fixture
4. 1 MiB conversation-like fixture
5. large corpus only after exact ID equality stays green

Scaling is forbidden until the previous rung passes.

## Tiny deterministic fixture

~~~text
Luna DU bridge: English y español, ñ, ação.
fn(x)=x+1
  two spaces	and tab.
~~~

The fixture intentionally includes ASCII, UTF-8, Spanish, Portuguese,
newline, repeated spaces, tab, and code-like text.

## Models

Initial targets:

1. Mistral local model
2. Qwen local model

Use the tokenizer embedded in or paired with the exact runtime model.

## Timed lanes

Record at least five independent timings.

### Lane A — Native tokenizer

~~~text
bytes -> native tokenizer -> native IDs
~~~

Measure tokenizer wall time, token count, packed ID SHA-256, and decoded
byte SHA-256.

### Lane B — Direct supplied IDs

~~~text
precomputed native IDs -> model runtime
~~~

Measure ID load/submit time, prefill time, generation time, and total
request wall.

### Lane C — DU to Direct-ID

~~~text
DU IDs -> Nereids Direct-ID adapter -> native IDs
~~~

Measure DU read time, repair/tokenization time, adapter wall time,
packed ID SHA-256, and first divergence.

### Lane D — Text prompt inference

~~~text
text prompt -> runtime tokenizer -> prefill -> generation
~~~

Measure tokenizer time if exposed, prefill time, generation time, total
request wall, output token IDs, and output text SHA-256.

### Lane E — DU/Nereids inference

~~~text
DU IDs -> Nereids Direct-ID -> native IDs -> prefill -> generation
~~~

Measure DU-to-native-ID time, prefill time, generation time, total request
wall, output token IDs, and output text SHA-256.

### Lane F — Experimental noncanonical DU vocabulary path

~~~text
DU IDs -> target-vocabulary IDs -> model runtime
~~~

This lane is experimental only.

It may use valid model-vocabulary IDs and reconstruct the exact bytes,
but if the IDs differ from the native tokenizer IDs then it is not
Direct-ID and not drop-in equivalent.

This lane can be explored for research or quality experiments, but its
results must be labeled noncanonical.

## Required equality gates

For Direct-ID model equivalence:

~~~text
Lane A native IDs == Lane C Nereids IDs
Lane B supplied IDs == Lane D tokenizer IDs
Lane D generated IDs == Lane E generated IDs
~~~

Use deterministic generation:

- fixed seed
- temperature 0 or greedy mode
- same context size
- same BOS/EOS policy
- same chat template policy
- same backend when checking strict equality

## Quality claim boundary

If Direct-ID equality passes, model input is the same as native tokenizer
input. Quality should be identical by construction.

Do not claim better quality from Direct-ID.

A noncanonical DU-derived segmentation may be tested as an experimental
variant, but it is not Direct-ID and cannot be described as drop-in
equivalent.

## Table shape

| Model | Path | Prep ms | Prefill ms | Gen ms | Total ms | Prompt IDs | Output IDs exact | Receipt |
|---|---:|---:|---:|---:|---:|---:|---|---|
| Mistral | text tokenizer | TBD | TBD | TBD | TBD | TBD | baseline | TBD |
| Mistral | direct IDs | TBD | TBD | TBD | TBD | same | yes/no | TBD |
| Mistral | DU -> Direct-ID | TBD | TBD | TBD | TBD | same | yes/no | TBD |
| Mistral | DU noncanonical | TBD | TBD | TBD | TBD | different | experimental | TBD |
| Qwen | text tokenizer | TBD | TBD | TBD | TBD | TBD | baseline | TBD |
| Qwen | direct IDs | TBD | TBD | TBD | TBD | same | yes/no | TBD |
| Qwen | DU -> Direct-ID | TBD | TBD | TBD | TBD | same | yes/no | TBD |
| Qwen | DU noncanonical | TBD | TBD | TBD | TBD | different | experimental | TBD |

## Do not mix metrics

Tokenizer throughput, DU translation throughput, prefill throughput, and
generation throughput are separate measurements.

No benchmark result is valid unless the receipt records which stage was
timed.
