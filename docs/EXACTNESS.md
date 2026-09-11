# Exactness

Exact:
- byte identity
- fixture rebuild when receipt proves it

Deterministic when supported:
- tokenizer/GGUF metadata parsing
- tensor location/indexing

Approximate:
- source-vector -> DU-vector -> target-vector projection

Rule:
Do not average packed quantized GGUF bytes.
Dequantize first, compose in f32/f16, then store/requantize later.
