# Direct-ID llama-server receipts — 2026-09-15

## Status

These receipts test llama-server transport and DU-restored inference gates
for Qwen and Mistral-Nemo.

They do not yet implement the optimized DU-ID to native-ID adapter.

## Source state

- Nereids HEAD: `c3e794fad740bf07802e1e63994ef8781188b000`
- Working tree status at write time:

~~~text
clean
~~~

## Runtime

- llama-server: `/mnt/data/OKC/llama.cpp/build-vulkan/bin/llama-server`
- Qwen model: `/mnt/data/shared_win_vms/LLMs/lmstudio-community/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf`
- Qwen SHA-256: `cd76ec205963b3b33350093e6904d9de16c4e666fd104e1f632d25c7f15f2a13`
- Mistral-Nemo model: `/mnt/winro/LLMs/lmstudio-community/Mistral-Nemo-Instruct-2407-GGUF/Mistral-Nemo-Instruct-2407-Q3_K_L.gguf`
- Mistral-Nemo SHA-256: `b4264ab33a58cf446eccd3ae55cfa5bba3d02f9cf5616bea2e6e657debd8975a`

## Fixtures

Tiny fixture:

~~~text
Luna DU bridge: English y español, ñ, ação.
fn(x)=x+1
  two spaces	and tab.
~~~

| Fixture | Bytes | SHA-256 |
|---|---:|---|
| tiny | 80 | `b0fb3d4680da46e97e113c1bb6a9374bf946ecb00d14a6739593d21976739571` |
| 4K repeated seed | 4096 | `586ae0733b4de709ff9835a5444a9af950c2dbab00353604b8d49631e3f2696f` |
| ctx-fill Qwen | 10212 | see receipt |
| ctx-fill Mistral-Nemo | 10574 | see receipt |

The 4K fixture is the tiny seed repeated and UTF-8-safe trimmed.

## Native tokenizer / detokenize / ID-array probe

Receipt directory:

~~~text
receipts/direct-id-ladder-probe-20260915-161401
~~~

| model | fixture_bytes | policy | tokens | tok_ms | detok | ids_prompt |
| --- | --- | --- | --- | --- | --- | --- |
| mistral_nemo_q3 | 4096 | default | 1434 | 4.361 | True | True |
| mistral_nemo_q3 | 4096 | no_special | 1434 | 3.117 | True | True |
| mistral_nemo_q3 | 4096 | with_special | 1435 | 2.868 | False | True |
| mistral_nemo_q3 | 65536 | default | 22938 | 28.564 | True | False |
| mistral_nemo_q3 | 65536 | no_special | 22938 | 28.234 | True | False |
| mistral_nemo_q3 | 65536 | with_special | 22939 | 26.208 | False | False |
| mistral_nemo_q3 | 80 | default | 28 | 0.518 | True | True |
| mistral_nemo_q3 | 80 | no_special | 28 | 0.691 | True | True |
| mistral_nemo_q3 | 80 | with_special | 29 | 0.407 | False | True |
| qwen35_9b | 4096 | default | 1485 | 3.315 | True | True |
| qwen35_9b | 4096 | no_special | 1485 | 2.175 | True | True |
| qwen35_9b | 4096 | with_special | 1485 | 2.197 | True | True |
| qwen35_9b | 65536 | default | 23757 | 17.511 | True | False |
| qwen35_9b | 65536 | no_special | 23757 | 18.434 | True | False |
| qwen35_9b | 65536 | with_special | 23757 | 16.016 | True | False |
| qwen35_9b | 80 | default | 29 | 0.629 | True | True |
| qwen35_9b | 80 | no_special | 29 | 0.672 | True | True |
| qwen35_9b | 80 | with_special | 29 | 0.471 | True | True |


Notes:

- `/tokenize` produced stable native IDs.
- `/detokenize` reconstructed the original bytes for the no-special/default
  policies used in later gates.
- llama-server accepts native prompt ID arrays through `prompt: [ids...]`.
- `tokens: [...]` is not accepted by this server.

## Text prompt vs direct ID prompt

Initial receipt directory:

~~~text
receipts/direct-id-completion-text-vs-ids-20260915-161742
~~~

Repeat receipt directory:

~~~text
receipts/hard-queue-directid-du-20260915-162807
~~~

| model | bytes | tokens | lane | tok_ms | completion_ms | output_equal | ids_sha |
| --- | --- | --- | --- | --- | --- | --- | --- |
| mistral_nemo_q3 | 4096 | 1434 | text_prompt | 3.364 | 7194.777 | True | 077cceb1a8a8... |
| mistral_nemo_q3 | 4096 | 1434 | direct_id_prompt_array | 3.364 | 6841.802 | True | 077cceb1a8a8... |
| mistral_nemo_q3 | 80 | 28 | text_prompt | 1.173 | 4049.512 | False | ce4054570a62... |
| mistral_nemo_q3 | 80 | 28 | direct_id_prompt_array | 1.173 | 4005.431 | False | ce4054570a62... |
| qwen35_9b | 4096 | 1485 | text_prompt | 1.691 | 6607.57 | True | ef697c4ad2c7... |
| qwen35_9b | 4096 | 1485 | direct_id_prompt_array | 1.691 | 6813.781 | True | ef697c4ad2c7... |
| qwen35_9b | 80 | 29 | text_prompt | 0.582 | 4039.167 | True | 01b67b10d9c1... |
| qwen35_9b | 80 | 29 | direct_id_prompt_array | 0.582 | 3952.643 | True | 01b67b10d9c1... |


Interpretation:

- The 4K text prompt and direct ID prompt produced identical deterministic
  output for both Qwen and Mistral-Nemo.
- Direct ID prompt transport is not a speed win through HTTP JSON by itself.
  The server still parses a JSON array of token IDs, and model inference
  dominates the total wall time.
- This is a transport and parity receipt, not an optimized runtime receipt.

## DU-restored ID inference gate

Receipt directory:

~~~text
receipts/du-restored-id-infer-gate-20260915-163830
~~~

| model | bytes | tokens | restore | ids | text_ms | du_id_ms | output_equal |
| --- | --- | --- | --- | --- | --- | --- | --- |
| mistral_nemo_q3 | 4096 | 1434 | True | True | 7257.443 | 6815.179 | True |
| mistral_nemo_q3 | 80 | 28 | True | True | 4055.553 | 4000.584 | False |
| qwen35_9b | 4096 | 1485 | True | True | 6279.447 | 6884.271 | True |
| qwen35_9b | 80 | 29 | True | True | 4103.257 | 4059.539 | True |


Interpretation:

- DU restored bytes matched the source fixtures exactly.
- Tokenizing the DU-restored bytes produced the same native IDs as tokenizing
  the original fixture bytes.
- For 4K prompts, DU-restored native IDs were accepted by llama-server as
  direct prompt ID arrays and produced matching deterministic output for both
  Qwen and Mistral-Nemo.
- Mistral-Nemo tiny output mismatch is kept as a policy/runtime edge receipt;
  it does not invalidate the 4K transport gate.

## Near-context fill timing

Receipt directory:

~~~text
receipts/ctx-fill-text-vs-direct-ids-20260915-164404
~~~

Target prompt length was 3700 native tokens under `ctx-size=4096` and
`n_predict=24`.

| model | bytes | tokens | tok_ms | text_ms | ids_ms | delta_ms | output_equal |
| --- | --- | --- | --- | --- | --- | --- | --- |
| mistral_nemo_q3 | 10574 | 3700 | 10.337 | 12814.968 | 12796.916 | -18.052 | True |
| qwen35_9b | 10212 | 3700 | 4.228 | 11994.615 | 11823.468 | -171.147 | True |


Interpretation:

- Near-context 3700-token prompts produced matching deterministic output
  for text prompt and direct ID prompt paths on both models.
- The measured direct-ID delta is small compared with total completion time.
- Any serious speed claim requires a lower-level or cached path that separates
  tokenization, DU translation, prefill, and generation.

## Current safe claims

~~~text
SAFE:
llama-server accepts native prompt ID arrays for Qwen3.5-9B and Mistral-Nemo.
Native tokenizer and detokenize receipts are exact for the tested fixtures.
DU-restored bytes retokenize to the same native IDs as original bytes.
4K and near-context direct-ID prompt paths match text prompt output.

NOT SAFE YET:
DU IDs -> native IDs without native tokenizer fallback.
Optimized Direct-ID adapter.
Better quality than the native tokenizer.
KV-cache portability.
Hidden-state portability.
Proteus release.
~~~

## Claim boundary

Direct-ID means:

~~~text
DU-origin bytes -> exact native model IDs -> same model input
~~~

Experimental DU-vocabulary lanes may test alternative segmentation, but if
their IDs differ from the native tokenizer IDs, they are not Direct-ID and
must be labeled experimental.

## Next gates

1. Implement a fixture-level Direct-ID adapter with full-window repair fallback.
2. Prove `native_ids == nereids_ids` for Qwen and Mistral-Nemo tiny/4K.
3. Add bounded repair-tail logic.
4. Run direct llama-server inference from Nereids-produced native IDs.
5. Only then scale beyond fixture tests.
