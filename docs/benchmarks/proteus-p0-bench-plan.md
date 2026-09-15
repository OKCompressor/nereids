# Proteus P0 benchmark plan

Status: plan only. No performance claims or receipts exist yet.

## Question

Measure how much native tokenizer work the exact snapshot/delta path avoids,
without mixing that result with inference, model prefill, storage policy, or
chat rendering.

Every measured append must first pass:

```text
full_tokenize(snapshot_bytes || delta_bytes)
==
incremental_candidate_ids
```

Failed equality attempts are diagnostic receipts, not benchmark samples.

## Transcript lanes

Use deterministic, conversation-like, already-rendered UTF-8 transcripts at
three prefix sizes:

| Lane | Exact target prefix size | Purpose |
|---|---:|---|
| 4K | 4 KiB (4,096 bytes) | small interactive baseline |
| 64K | 64 KiB (65,536 bytes) | medium archived conversation |
| 1 MiB | 1,048,576 bytes | large persistence/resume lane |

Define and retain an explicit deterministic truncation/padding rule so each
receipt records the exact target byte count, actual byte count, SHA-256, turn
boundaries, delta bytes, and delta SHA-256. Use the same prefix and appended
turn for all paths in one comparison. Rendering is setup work and stays
outside timed regions.

## Separate measurement paths

1. **Cold full-history tokenization**: start from no tokenizer-side prefix
   state and tokenize all `P || D` bytes. Record only tokenizer wall time.
2. **Warm in-memory incremental append**: retain a validated
   `ProteusSnapshotV0` in memory, append `D`, rebuild exact native spans and DU
   layers, and serialize the next snapshot. Deserialization time is zero and
   must be reported as such.
3. **Cold snapshot reload plus incremental append**: begin with persisted
   snapshot bytes, deserialize, validate, append `D`, rebuild exact spans and
   layers, and serialize the next snapshot.
4. **Model inference timing**: run as a separate experiment with model,
   runtime, sampling, context, prefix/prefill policy, and KV policy pinned.

Never add tokenizer/resume time and inference latency into one claimed saving.
Never attribute prefix-prefill or KV-cache behavior to this P0 proof.

## Required timing fields

Each cold or warm incremental receipt reports:

```text
snapshot_deserialize_wall_ns
snapshot_validate_wall_ns
delta_du_reconstruct_wall_ns
direct_id_candidate_prep_wall_ns
native_span_update_wall_ns
dictionary_layer_update_wall_ns
snapshot_serialize_wall_ns
proteus_resume_total_wall_ns
oracle_full_tokenize_wall_ns
```

`proteus_resume_total_wall_ns` ends after next-snapshot serialization and
excludes the later oracle call. The oracle field measures only the full native
tokenizer call; transcript generation and DU preparation are setup. Component
timings may overlap with nested Nereids validation/reconstruction and must not
be summed as if they were disjoint. The end-to-end resume total is the primary
incremental wall measurement.

## Procedure and receipts

- Pin Nereids commit, Proteus source commit or patch hash, Rust toolchain,
  llama.cpp build, model weights, tokenizer files, host, CPU affinity, and
  power/performance policy.
- Warm the tokenizer endpoint before recorded warm runs. For cold runs, state
  exactly which process, filesystem cache, and network conditions are reset.
- Use one fixed repair depth selected before measured trials. Do not search
  repair depths with oracle IDs inside a benchmark sample.
- Alternate path order or randomize it with a recorded seed. Record raw trials
  rather than only aggregates.
- Run enough repetitions to report count, median, p5, p95, minimum, and
  maximum. Do not discard outliers without a stated rule and retained raw data.
- Record candidate/oracle ID counts and SHA-256 over little-endian `u32` IDs,
  equality, repair boundary, repair-window byte count, dictionary lineage,
  append sequence, and newly allocated dictionary-entry count.
- Keep inference results in distinct receipts and tables.

Only receipts produced under this plan may support later README benchmark
claims.
