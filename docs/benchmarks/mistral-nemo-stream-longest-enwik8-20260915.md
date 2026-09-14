# Mistral Nemo stream-longest Rust projection — enwik8 100 MB

Date: 2026-09-15  
Implementation commit: `27a7eaaceb27dd51cd8cdd5de5402cd53723845f`

## Scope

This benchmark measures Nereids' continuous cross-boundary longest-byte-match
projection from raw bytes into the preserved native Mistral Nemo / Tekken-v3
vocabulary.

It is a structural byte projection.

It does **not** claim canonical Tekken segmentation or model-state equivalence.

## Inputs

- Corpus: enwik8
- Bytes: `100,000,000`
- enwik8 SHA-256:
  `2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8`
- Target vocabulary entries: `131,072`
- Maximum target piece length: `76` bytes
- `model.bin` SHA-256:
  `94b7ab2b1fcfac9e6960ed12bf797d398a72d3156746383d6f433319395b6a66`

## Rust stream-longest result

- Native target IDs: `27,179,443`
- Bytes / target ID: `3.6792512635`
- Target-ID u32le SHA-256:
  `19564cca8a6eb336e7aa8e314acbbc1379e1996fd58682b05e92adb50b99d977`
- Exact byte reconstruction: **true**
- Warmups: `1`
- Measured repetitions: `5`
- Median wall time: `2.641436894 s`
- Median throughput: `37.8581825 MB/s`
- Peak RSS: `344,288 KiB`

Measured wall times:

- `2.613941679 s`
- `2.641436894 s`
- `2.680512911 s`
- `2.605153108 s`
- `2.651711369 s`

Receipt SHA-256:

`da33d93fbf3b44ffd5862e4c4c1de8740947405122e3e37ff4f22eed1f8e9cc0`

## Canonical Tekken comparison

The previously measured official canonical Tekken segmentation of the same
100,000,000-byte enwik8 corpus contains:

- Canonical Tekken IDs: `27,290,089`
- Canonical bytes / ID: `3.6643339639`

The Nereids stream-longest view emits:

- `110,646` fewer positions
- `0.4054439%` fewer positions relative to canonical Tekken

This is a segmentation result, not a canonical-equivalence claim.

Both streams reconstruct the same source bytes, but the token boundaries and
therefore model computation may differ.

## Trie equivalence gate

Before the full-corpus run, the optimized Rust trie was checked against the
existing four-lane stream-longest reference on the exact 8,356-byte fixture.

Both produced:

- `2,605` IDs
- u32le SHA-256:
  `f7ce468d9fd96b60ccef07637cf57657e88c5a111b1ccd00b80fadf47710a9b2`
- exact byte reconstruction

The previous Rust linear-bucket implementation had a median of
`176.506556 ms` on this fixture.

The trie implementation had a median of `0.187242 ms`.

That is a local implementation speedup of approximately `942.7x` while
preserving the output ID stream exactly.

This number compares the old and new Nereids Rust lookup implementations.
It is **not** a tokenizer-vs-tokenizer speedup claim.

## Earlier timing context

| Path | Input | Time | Environment |
|---|---:|---:|---|
| Python stream-longest prototype (B) | 8,356 B | 25.000033 ms | PC2 |
| Official canonical Tekken (C) | 8,356 B | 1.828062 ms | PC2 |
| Nereids Rust trie stream-longest (B) | 8,356 B | 0.187242 ms median | PC1 |
| Official canonical Tekken | 100 MB | ~26.10 s wall | earlier oracle run |
| Nereids Rust trie stream-longest | 100 MB | 2.641436894 s median | PC1 |

The historical 100 MB timing ratio is approximately `9.88x`, but these are
not matched same-machine/same-harness measurements, so no tokenizer-speed
claim is made from that ratio.

## Claim boundary

Supported:

> On full 100 MB enwik8, Nereids Rust continuously projected the byte stream
> into valid Mistral Nemo/Tekken-v3 native vocabulary IDs with exact byte
> reconstruction. Its noncanonical stream-longest segmentation emitted
> 27,179,443 IDs, 0.405% fewer positions than the separately measured
> canonical Tekken segmentation.

Not claimed:

- canonical Tekken equivalence
- identical model computation
- tokenizer replacement equivalence
- a matched performance speedup over Tekken/tiktoken

Bytes are identity. Structure is reusable. Tokenizers are views.
