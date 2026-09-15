# Proteus P0 Roadmap

## Purpose

Proteus begins after Nereids can produce canonical native model IDs.

Nereids:

~~~text
DU / structural state -> native tokenizer IDs
~~~

Proteus:

~~~text
snapshot + delta -> reusable prefix + repair tail + native delta IDs
-> model I/O orchestration
~~~

## P0 exact gate

Given:

- prefix bytes `P`
- delta bytes `D`
- model tokenizer `T_model`

Proteus P0 passes only when:

~~~text
T_model(P || D)
==
Proteus_Nereids_incremental_ids(P_snapshot, D)
~~~

This must be tested for each target model tokenizer.

Initial targets:

1. Mistral
2. Qwen

## P1 model gate

After P0 ID equality:

~~~text
full text path output IDs
==
snapshot/delta path output IDs
~~~

Use deterministic generation and the same runtime backend.

## P2 performance gate

Measure avoided work separately:

- avoided tokenization
- avoided prefix prefill
- reused KV cache
- delta repair time
- total request wall

## KV-cache boundary

KV-cache reuse is exact only for:

- same model
- same weights
- same tokenizer IDs
- same runtime policy
- same prefix
- compatible backend behavior

KV and hidden states are model-specific. Cross-model or learned
projection is approximate unless separately proven.

## Infinite conversation boundary

Proteus does not make a finite-context model infinite.

Allowed claim:

~~~text
conversation history can be stored as exact structural state;
active model context remains bounded by the model;
snapshot/delta logic can avoid recomputing unchanged prefixes.
~~~

Not allowed yet:

~~~text
infinite context
lossless hidden-state transfer
KV portability across models
~~~

## Release gates

No Proteus release before:

1. Mistral P0 Direct-ID delta equality
2. Qwen P0 Direct-ID delta equality
3. deterministic text-vs-delta model output check
4. receipt schema
5. at least one latency table
