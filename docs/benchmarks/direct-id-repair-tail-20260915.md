# Direct-ID append repair receipts — 2026-09-15

## Purpose

Measure how much of a native-token snapshot must be repaired when new
bytes are appended, while requiring exact equality with full native
tokenization.

These receipts establish empirical repair behavior for Qwen3.5-9B and
Mistral-Nemo. They do not establish a universal tokenizer bound.

## Long-fixture repair-tail probe

Fixture size: 10,212 bytes.

| Model | Delta B | Prefix tokens | Tail repaired B | Tokens dropped | Full tokenize ms | Repair tokenize ms | Tokenizer ms saved | Tokenizer work saved |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| qwen35_9b | 16 | 3693 | 11 | 3 | 18.508 | 0.478 | 18.030 | 97.4% |
| qwen35_9b | 64 | 3679 | 9 | 2 | 18.508 | 0.436 | 18.072 | 97.6% |
| qwen35_9b | 256 | 3606 | 11 | 3 | 18.508 | 0.516 | 17.992 | 97.2% |
| qwen35_9b | 1024 | 3331 | 9 | 2 | 18.508 | 0.891 | 17.617 | 95.2% |
| mistral_nemo_q3 | 16 | 3566 | 11 | 3 | 24.551 | 0.720 | 23.831 | 97.1% |
| mistral_nemo_q3 | 64 | 3552 | 9 | 2 | 24.551 | 0.605 | 23.945 | 97.5% |
| mistral_nemo_q3 | 256 | 3482 | 11 | 3 | 24.551 | 0.749 | 23.802 | 96.9% |
| mistral_nemo_q3 | 1024 | 3216 | 9 | 2 | 24.551 | 1.182 | 23.369 | 95.2% |

### Timing interpretation

The timing table measures tokenizer calls only:

~~~text
full native tokenize(prefix || delta)
versus
native tokenize(repair_tail || delta)
~~~

The percentage is therefore tokenizer-work reduction, not an end-to-end
Direct-ID V1 latency claim.

It excludes snapshot lookup, boundary bookkeeping, splice work, HTTP
transport, verification-oracle work, and model inference.

The current research probe also used detokenization while searching for
repair boundaries. A production snapshot should retain exact byte-span
metadata so that repeated detokenization is not required.

## Adversarial append matrix

| Model | Cases | All exact | Max dropped tokens | Max repaired tail B | Drop 0 | Drop 1 | Drop 2 | Other |
|---|---:|---|---:|---:|---:|---:|---:|---:|
| qwen35_9b | 72 | True | 1 | 128 | 55 | 17 | 0 | 0 |
| mistral_nemo_q3 | 72 | True | 2 | 64 | 54 | 15 | 3 | 0 |

Combined result:

~~~text
144 adversarial append cases
144 exact canonical native-ID reconstructions

Qwen3.5:
  observed maximum minimum drop = 1 snapshot token
  observed maximum repaired tail = 128 bytes

Mistral-Nemo:
  observed maximum minimum drop = 2 snapshot tokens
  observed maximum repaired tail = 64 bytes
~~~

The byte maxima and token maxima are observations from this test matrix,
not universal bounds.

## Architectural consequence

The measured behavior supports an adaptive token-tail repair design:

~~~text
native snapshot IDs
+ appended DU-origin delta bytes
-> preserve stable snapshot prefix
-> repair only the mutable token tail + delta
-> splice native IDs
-> verify against full native tokenization
~~~

A fixed byte bound should not be assumed. One observed Qwen tail token
covered 128 bytes, so snapshot token boundaries are the more useful
repair primitive.

## Receipt hashes

- Long-probe summary SHA-256: `af873568c960a94a98a8013fea7a6cab0173ccdd2449ec7217f41e8e4fe6c9d9`
- Adversarial summary SHA-256: `f5ff69c2357cc8b0646bd1bbacacadeb7315cb2243c5e47935d6a0a554837310`

Receipt directories:

- `receipts/direct-id-repair-tail-probe-20260915-171127`
- `receipts/direct-id-repair-tail-adversarial-20260915-171346`

## Safe claim

For the tested append cases, retaining the stable native-ID prefix and
retokenizing only a small suffix plus the appended delta was sufficient
to reconstruct exactly the same native IDs as full native tokenization.

## Not yet claimed

~~~text
1 or 2 dropped tokens is a universal bound
64 or 128 bytes is a universal bound
V1 end-to-end latency savings
native tokenizer verification can already be removed
Proteus P0 is complete
~~~
