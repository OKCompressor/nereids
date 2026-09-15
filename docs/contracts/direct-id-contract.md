# Nereids Direct-ID Contract v0

## Purpose

Direct-ID is the stronger Nereids contract for model I/O.

Phase 3 proves byte-exact target-vocabulary projection:

~~~text
DU IDs -> target-vocabulary IDs -> exact original bytes
~~~

Direct-ID requires native tokenizer identity:

~~~text
DU IDs -> native model tokenizer IDs
~~~

The adapter passes only when the Nereids-produced IDs are exactly the
same IDs that the model tokenizer would produce for the same bytes under
the same tokenization policy.

## Definitions

Let:

- `B` be the canonical raw byte sequence.
- `T_model` be the exact tokenizer used by the target model runtime.
- `native_ids = T_model(B)`.
- `DU(B)` be the DU3 structural representation of `B`.
- `nereids_ids = Adapter(DU(B), T_model_policy)`.

## Required policy inputs

Every Direct-ID receipt must record:

- model name/path
- tokenizer source
- tokenizer hash or model metadata hash
- BOS policy
- EOS policy
- chat template policy
- special-token policy
- byte normalization policy
- UTF-8 policy
- fixture byte SHA-256

No hidden normalization is allowed.

## Pass conditions

A Direct-ID adapter passes for a fixture only when all conditions hold:

1. `native_ids == nereids_ids`
2. `decode(native_ids) == B`
3. `decode(nereids_ids) == B`
4. token count is identical
5. packed ID SHA-256 is identical
6. first divergence is absent
7. BOS/EOS/template policy is explicit and identical

## Byte-exact projection vs Direct-ID

Byte-exact projection:

~~~text
IDs are valid in the target vocabulary.
Decoded bytes equal the original bytes.
Native canonical segmentation is not claimed.
~~~

Direct-ID:

~~~text
IDs equal the exact native tokenizer output.
Decoded bytes equal the original bytes.
Model input equivalence is claimed.
~~~

Only Direct-ID supports drop-in model-tokenizer replacement claims.

## Boundary rule

Native tokenizers may merge across DU lexeme boundaries.

Therefore a correct Direct-ID adapter must handle boundaries explicitly.

Allowed strategies:

1. joint-safe boundaries
2. bounded repair tail
3. full-window repair for small fixtures
4. tokenizer-oracle fallback with receipt labeling

A per-DU-lexeme greedy map is not sufficient to claim canonical native
ID equality.

## Minimal receipt fields

~~~json
{
  "schema": "nereids-direct-id-receipt-v0",
  "model": "...",
  "tokenizer": "...",
  "fixture": "...",
  "fixture_bytes": 0,
  "fixture_sha256": "...",
  "native_token_count": 0,
  "nereids_token_count": 0,
  "native_ids_sha256": "...",
  "nereids_ids_sha256": "...",
  "ids_exact": false,
  "native_decode_exact": false,
  "nereids_decode_exact": false,
  "first_divergence": null,
  "bos_policy": "...",
  "eos_policy": "...",
  "template_policy": "...",
  "tokenization_wall_ns": 0,
  "du_to_direct_id_wall_ns": 0,
  "notes": []
}
~~~

## Non-claims

This contract does not claim:

- better model quality
- KV-cache portability
- hidden-state portability
- canonical equality across different tokenizers
- learned projection exactness

KV and hidden-state reuse require separate Proteus receipts.
