# Direct-ID V1 bounded append repair — 2026-09-15

## Result

The Rust V1 implementation reused native snapshot IDs, repaired only an
adaptive suffix plus appended delta, and matched full native tokenization
exactly across the acceptance matrix.

| Model | Cases | Exact | Max observed drop | Max observed tail | Median repair tokenize ms | Median full oracle ms | Median tokenizer work saved |
|---|---:|---|---:|---:|---:|---:|---:|
| Qwen3.5-9B | 72 | True | 1 tokens | 128 B | 0.240 | 3.950 | 92.719% |
| Mistral-Nemo | 72 | True | 2 tokens | 64 B | 0.508 | 6.402 | 92.692% |

Combined acceptance:

~~~text
144 cases
144 exact native-ID reconstructions
0 failures
candidate_used_full_window=false
oracle_used_for_verification_only=true
~~~

## Timing boundary

The reported percentage compares only:

~~~text
candidate repair-window native-tokenizer call
versus
oracle full native-tokenizer call
~~~

It is tokenizer-work savings, not total Direct-ID preparation latency and
not model inference latency.

Candidate bookkeeping, detokenization used by the current fixture search,
HTTP overhead, splice work, and oracle verification are not represented
by that percentage.

A production snapshot should retain exact token byte spans so boundary
lookup does not require repeated detokenization.

## Observed repair bounds

~~~text
Qwen3.5-9B:
  maximum observed minimum drop: 1 snapshot token
  maximum observed snapshot tail: 128 bytes

Mistral-Nemo:
  maximum observed minimum drop: 2 snapshot tokens
  maximum observed snapshot tail: 64 bytes
~~~

These are empirical maxima from this matrix, not universal tokenizer
bounds.

## Worst observed repair cases

| Model | Base | Delta | Drop tokens | Tail B | Repair ms | Oracle ms | Tokenizer work saved |
|---|---|---|---:|---:|---:|---:|---:|
| mistral_nemo_q3 | long_ascii_word | utf8 | 2 | 4 | 1.583 | 15.647 | 89.9% |
| mistral_nemo_q3 | long_ascii_word | word64 | 2 | 4 | 1.755 | 13.197 | 86.7% |
| mistral_nemo_q3 | long_ascii_word | x | 2 | 4 | 0.794 | 9.196 | 91.4% |
| qwen35_9b | spaces | digits | 1 | 128 | 0.476 | 4.122 | 88.5% |
| qwen35_9b | spaces | punct | 1 | 128 | 0.477 | 4.246 | 88.8% |
| qwen35_9b | spaces | quote | 1 | 128 | 0.501 | 4.196 | 88.0% |
| qwen35_9b | spaces | utf8 | 1 | 128 | 0.649 | 4.482 | 85.5% |
| qwen35_9b | spaces | word64 | 1 | 128 | 0.753 | 4.954 | 84.8% |
| qwen35_9b | spaces | x | 1 | 128 | 0.620 | 4.135 | 85.0% |
| mistral_nemo_q3 | spaces | digits | 1 | 64 | 0.829 | 4.948 | 83.3% |
| mistral_nemo_q3 | spaces | punct | 1 | 64 | 0.829 | 5.118 | 83.8% |
| mistral_nemo_q3 | spaces | quote | 1 | 64 | 0.842 | 4.946 | 83.0% |
| mistral_nemo_q3 | spaces | utf8 | 1 | 64 | 0.864 | 6.652 | 87.0% |
| mistral_nemo_q3 | spaces | word64 | 1 | 64 | 1.025 | 5.173 | 80.2% |
| mistral_nemo_q3 | spaces | x | 1 | 64 | 1.001 | 6.801 | 85.3% |
| qwen35_9b | long_ascii_word | word64 | 1 | 8 | 0.492 | 6.640 | 92.6% |
| qwen35_9b | long_ascii_word | x | 1 | 8 | 0.443 | 6.846 | 93.5% |
| mistral_nemo_q3 | newlines | newline | 1 | 4 | 0.559 | 3.475 | 83.9% |
| mistral_nemo_q3 | code | newline | 1 | 3 | 0.641 | 6.460 | 90.1% |
| mistral_nemo_q3 | jsonish | newline | 1 | 2 | 0.502 | 5.220 | 90.4% |

## V1 architecture

~~~text
native snapshot IDs
+ appended delta bytes
-> retain stable snapshot-ID prefix
-> retokenize only mutable snapshot tail + delta
-> splice repaired native IDs
-> compare with full native-tokenizer oracle
~~~

The candidate path does not construct its result by tokenizing the full
non-empty prefix plus delta.

## Receipt hashes

- `summary.csv`: `2a36c3ca8c67dce1e9977981f9707f68313a99edd1b8f6c389971912de5c00ff`
- `summary.json`: `7b9b5dbaa25ff7ae07d1d616fc2d28b61b416517e337861abafb179978a1037d`

Receipt directory:

- `receipts/direct-id-v1-acceptance-20260915-173511`

## Safe claim

For these 144 tested append cases across Qwen3.5-9B and Mistral-Nemo,
Nereids reused the stable native-ID snapshot prefix, retokenized only an
adaptive suffix plus delta, and produced exactly the same canonical
native IDs as full native tokenization.

## Not yet claimed

~~~text
1 or 2 tokens is a universal repair bound
64 or 128 bytes is a universal repair bound
92.7% is end-to-end Direct-ID latency savings
native verification oracle can already be removed
KV-cache reuse is proven
Proteus P0 is complete
~~~
