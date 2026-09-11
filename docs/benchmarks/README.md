# Nereids benchmark and receipt notes

Public benchmark notes summarize reproducible receipts without checking large generated artifacts into the source repository.

## Current notes

- [Phase 3 — exact DU3 → cl100k vocabulary projection at 1 GB](./enwik9-phase3-20260911.md)
  - corpus: `enwik9`
  - exact input: `1,000,000,000` bytes
  - DU3 → full `cl100k_base` mergeable vocabulary
  - exact reconstructed SHA-256
  - canonical BPE-equivalent segmentation is **not** claimed

## Related public surfaces

- [Nereids project](https://github.com/OKCompressor/nereids)
- [DU3 public release / receipts](https://github.com/OKCompressor/core/releases/tag/modules-alpha-20260911)
- [OKCompressor core](https://github.com/OKCompressor/core)
- [OKCompressor public workshop](https://github.com/OKCompressor/okc-www)
- [Public lab source](https://github.com/OKCompressor/okc-www/tree/main/www/lab)
- [Public research source](https://github.com/OKCompressor/okc-www/tree/main/www/research)

Raw benchmark receipts and large generated artifacts remain outside the source tree unless a release explicitly publishes them.

Source available. Receipts before claims.
