# Direct-ID V2 DU-aware acceptance — 2026-09-15

## Result

Direct-ID V2 passed **144/144** cases across the same 72-case
append matrix used for Qwen3.5-9B and Mistral-Nemo in Direct-ID V1.

The fixed repair policy was selected before each V2 candidate was built:

| Model | Fixed drop | Exact |
| --- | ---: | ---: |
| Qwen3.5-9B | 1 native token | 72/72 |
| Mistral-Nemo | 2 native tokens | 72/72 |

The oracle did not construct the candidate and did not select the repair
depth. Full native tokenization was used only after candidate construction
to verify canonical ID equality.

## V2 total preparation timing

`candidate_total_prep` includes DU reconstruction, stored-span boundary
lookup, repair-window native tokenization, and native-ID splice. Snapshot
load/validation, oracle tokenization, equality checking, and full candidate
detokenization are outside this timing.

| Model | DU reconstruct | Span lookup | Repair tokenize | Splice | **V2 total prep** | Full tokenize | **Median prep saved** |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Qwen3.5-9B | 0.001 ms | 0.000 ms | 0.336 ms | 0.005 ms | **0.351 ms** | 4.095 ms | **90.5%** |
| Mistral-Nemo | 0.001 ms | 0.000 ms | 0.520 ms | 0.009 ms | **0.529 ms** | 5.694 ms | **90.8%** |

The percentage above is the median of the per-case preparation savings,
not a percentage derived from independently aggregated medians.

## V1 → V2 measurement boundary

| Model | V1 median repair-tokenizer work saved | V2 median total-preparation saved |
| --- | ---: | ---: |
| Qwen3.5-9B | 92.7% | 90.5% |
| Mistral-Nemo | 92.7% | 90.8% |

V1 measured only the native tokenizer call. V2 measures the wider candidate
preparation path, so the two percentages are intentionally not equivalent.

## Exactness gates

Every passing fixed-drop case required:

~~~text
du_delta_reconstruction_exact=true
native_snapshot_spans_exact=true
ids_exact=true
decode_exact=true

repair_selection_mode=fixed_drop
oracle_used_for_candidate_construction=false
oracle_used_for_drop_selection=false
oracle_used_for_verification=true

candidate_used_full_window=false
candidate_used_full_prefix_detokenize=false
~~~

Native prefix spans were persisted from the model tokenizer's token IDs and
exact token pieces before candidate timing.

## DU state

Each prefix and each appended delta was encoded as an exact DU3-v1g state.
The small one-chunk runs were globalized with `merge-dicts` plus `build-gmap32`
so they use the same `merged.dict + local_u16 + gmap32` contract consumed by
`load_du_stream()`.

Delta dictionaries are independent in this V2 fixture. Persistent global-DU
dictionary evolution is intentionally deferred to Proteus.

## Observed repair sizes

| Model | Max snapshot tail | Max repair window | Max repair token count |
| --- | ---: | ---: | ---: |
| Qwen3.5-9B | 128 B | 403 B | 121 |
| Mistral-Nemo | 128 B | 403 B | 122 |

These are observations from this acceptance matrix, not universal tokenizer
bounds.

## Claim boundary

This receipt supports:

> For the tested append matrix, Nereids V2 reconstructed DU-origin bytes,
> selected the repair boundary from persisted native byte spans, tokenized
> only a fixed native-token suffix plus delta, and reproduced the canonical
> native model token IDs exactly.

It does **not** establish tokenizer-free DU-to-native-ID conversion, universal
repair-depth bounds, KV-cache portability, or an end-to-end inference speedup.

## Receipt

~~~text
receipts/direct-id-v2-acceptance-20260915-183907
~~~
