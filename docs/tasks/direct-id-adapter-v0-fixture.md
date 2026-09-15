# Task: Direct-ID Adapter V0 Fixture CLI

## Goal

Add a fixture-level Direct-ID CLI:

~~~text
nereids-direct-id-fixture
~~~

This is a correctness adapter, not the optimized speed path.

## Contract

Inputs:

~~~text
--fixture <path>
--du-restored <path>
--native-tokenizer-server <url>
--model-name <name>
--receipt <json>
~~~

Example:

~~~text
nereids-direct-id-fixture \
  --fixture receipts/.../fixtures/4k.txt \
  --du-restored receipts/.../du/4k/.../u16/restored \
  --native-tokenizer-server http://127.0.0.1:8091 \
  --model-name qwen35_9b \
  --receipt receipts/direct-id-v0-qwen-4k.json
~~~

## V0 semantics

V0 may use full-window repair fallback:

~~~text
DU restored bytes -> native tokenizer server -> nereids_ids
fixture bytes     -> native tokenizer server -> native_ids
~~~

Pass iff:

~~~text
fixture bytes == DU restored bytes
native_ids == nereids_ids
decode(native_ids) == fixture bytes, if /detokenize is available
decode(nereids_ids) == fixture bytes, if /detokenize is available
~~~

This proves the Direct-ID receipt shape and model-tokenizer policy.

It does not yet prove optimized DU-ID to native-ID remap without tokenizer
fallback.

## Required receipt fields

~~~json
{
  "schema": "nereids-direct-id-fixture-v0",
  "model_name": "...",
  "fixture": "...",
  "du_restored": "...",
  "fixture_bytes": 0,
  "fixture_sha256": "...",
  "du_restored_sha256": "...",
  "du_restore_exact": false,
  "native_token_count": 0,
  "nereids_token_count": 0,
  "native_ids_sha256_u32le": "...",
  "nereids_ids_sha256_u32le": "...",
  "ids_exact": false,
  "first_divergence": null,
  "token_policy": "add_special=false",
  "native_tokenize_wall_ns": 0,
  "nereids_full_window_repair_wall_ns": 0,
  "detokenize_available": false,
  "native_decode_exact": null,
  "nereids_decode_exact": null,
  "notes": []
}
~~~

## Implementation preference

Add:

~~~text
src/bin/nereids-direct-id-fixture.rs
~~~

Use existing project style.

Avoid touching unrelated files.

Do not modify llama.cpp.

No quality claims.

## Acceptance gate

Run against:

~~~text
Qwen3.5-9B 4K fixture
Mistral-Nemo Q3 4K fixture
~~~

Both must return:

~~~text
du_restore_exact=true
ids_exact=true
native_decode_exact=true or detokenize_available=false
nereids_decode_exact=true or detokenize_available=false
~~~
