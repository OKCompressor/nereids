#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import struct
import time
from pathlib import Path


def parse_len_prefixed_vocab(data: bytes, off: int, n_vocab: int) -> list[bytes]:
    vocab: list[bytes] = []
    for _ in range(n_vocab):
        if off + 4 > len(data):
            raise ValueError("truncated model.bin at length field")
        (n,) = struct.unpack_from("<I", data, off)
        off += 4
        if n > 1_000_000:
            raise ValueError(f"implausible token length {n}")
        if off + n > len(data):
            raise ValueError("truncated model.bin at token bytes")
        vocab.append(data[off:off+n])
        off += n

    if off != len(data):
        raise ValueError(f"trailing bytes: {len(data)-off}")

    return vocab


def load_model_bin(path: Path) -> tuple[list[bytes], str]:
    data = path.read_bytes()

    # Newer helper format: MB01 + u32 vocab + repeated u32 len + bytes.
    if len(data) >= 8 and data[:4] == b"MB01":
        (n_vocab,) = struct.unpack_from("<I", data, 4)
        return parse_len_prefixed_vocab(data, 8, n_vocab), "MB01"

    # Older local benchmark format: u32 vocab + repeated u32 len + bytes.
    if len(data) >= 4:
        (n_vocab,) = struct.unpack_from("<I", data, 0)
        if 1 <= n_vocab <= 10_000_000:
            try:
                return parse_len_prefixed_vocab(data, 4, n_vocab), "u32-count-no-magic"
            except Exception as e:
                no_magic_err = repr(e)
        else:
            no_magic_err = f"implausible n_vocab={n_vocab}"
    else:
        no_magic_err = "too small"

    raise ValueError(
        f"unknown model.bin format: first32={data[:32].hex()} no_magic_err={no_magic_err}"
    )


def read_u32s(path: Path):
    b = path.read_bytes()
    if len(b) % 4:
        raise ValueError(f"bad u32 file size: {path}")
    for (x,) in struct.iter_unpack("<I", b):
        yield x


def read_u16s(path: Path):
    b = path.read_bytes()
    if len(b) % 2:
        raise ValueError(f"bad u16 file size: {path}")
    for (x,) in struct.iter_unpack("<H", b):
        yield x


def load_gmap(path: Path, width: int) -> list[int]:
    b = path.read_bytes()

    if width == 24:
        if len(b) % 3:
            raise ValueError(f"bad gmap24 size: {path}")
        return [
            b[i] | (b[i + 1] << 8) | (b[i + 2] << 16)
            for i in range(0, len(b), 3)
        ]

    if width == 32:
        if len(b) % 4:
            raise ValueError(f"bad gmap32 size: {path}")
        return [x[0] for x in struct.iter_unpack("<I", b)]

    raise ValueError(width)


def load_du_dict(path: Path) -> list[str]:
    out: list[str] = []
    with path.open("r", encoding="utf-8", newline="") as f:
        for line in f:
            if line.endswith("\n"):
                line = line[:-1]
            if line.endswith("\r"):
                line = line[:-1]
            out.append(line)
    return out


class GreedyVocab:
    def __init__(self, vocab: list[bytes]):
        self.by_first: dict[int, list[tuple[bytes, int]]] = {}
        self.max_len = 0

        for tid, bs in enumerate(vocab):
            if not bs:
                continue
            self.by_first.setdefault(bs[0], []).append((bs, tid))
            self.max_len = max(self.max_len, len(bs))

        for k in self.by_first:
            self.by_first[k].sort(key=lambda x: len(x[0]), reverse=True)

    def encode_bytes(self, bs: bytes) -> list[int]:
        out: list[int] = []
        i = 0
        n = len(bs)

        while i < n:
            hit = None
            for tok, tid in self.by_first.get(bs[i], []):
                if bs.startswith(tok, i):
                    hit = (tok, tid)
                    break

            if hit is None:
                raise ValueError(f"no target vocab token at offset={i} byte=0x{bs[i]:02x}")

            tok, tid = hit
            out.append(tid)
            i += len(tok)

        return out


def first_mismatch_bytes(a: bytes, b: bytes) -> int | None:
    n = min(len(a), len(b))
    for i in range(n):
        if a[i] != b[i]:
            return i
    if len(a) != len(b):
        return n
    return None


def first_mismatch_ids(a: list[int], b: list[int]) -> int | None:
    n = min(len(a), len(b))
    for i in range(n):
        if a[i] != b[i]:
            return i
    if len(a) != len(b):
        return n
    return None


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--du-run", required=True)
    ap.add_argument("--raw", required=True)
    ap.add_argument("--tik-remap", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--limit-bytes", type=int, default=1_000_000)
    args = ap.parse_args()

    t0 = time.time()

    du_run = Path(args.du_run)
    raw = Path(args.raw)
    tik = Path(args.tik_remap)
    outdir = Path(args.out)
    outdir.mkdir(parents=True, exist_ok=True)

    target_vocab, model_format = load_model_bin(tik / "model.bin")
    greedy = GreedyVocab(target_vocab)

    du_vocab = load_du_dict(du_run / "merged.dict")
    local_dir = du_run / "local_u16"

    if (du_run / "gmap24").is_dir():
        gmap_dir = du_run / "gmap24"
        gmap_width = 24
        gmap_suffix = ".gmap24"
    elif (du_run / "gmap32").is_dir():
        gmap_dir = du_run / "gmap32"
        gmap_width = 32
        gmap_suffix = ".gmap32"
    else:
        raise SystemExit("missing gmap24/gmap32")

    bridge_ids: list[int] = []
    du_parts: list[bytes] = []
    du_positions = 0
    byte_len = 0
    cache: dict[int, list[int]] = {}

    for u16_path in sorted(local_dir.glob("*.u16")):
        stem = u16_path.name[:-4]
        gmap = load_gmap(gmap_dir / f"{stem}{gmap_suffix}", gmap_width)

        for lid in read_u16s(u16_path):
            gid = gmap[lid]
            tok = du_vocab[gid].encode("utf-8")

            du_parts.append(tok)
            byte_len += len(tok)

            ids = cache.get(gid)
            if ids is None:
                ids = greedy.encode_bytes(tok)
                cache[gid] = ids

            bridge_ids.extend(ids)
            du_positions += 1

            if byte_len >= args.limit_bytes:
                break

        if byte_len >= args.limit_bytes:
            break

    du_bytes = b"".join(du_parts)
    raw_prefix = raw.read_bytes()[:len(du_bytes)]
    bridge_bytes = b"".join(target_vocab[i] for i in bridge_ids)

    direct_ids: list[int] = []
    direct_parts: list[bytes] = []
    direct_len = 0

    for tid in read_u32s(tik / "tokens.u32"):
        direct_ids.append(tid)
        bs = target_vocab[tid]
        direct_parts.append(bs)
        direct_len += len(bs)
        if direct_len >= len(raw_prefix):
            break

    direct_bytes = b"".join(direct_parts)
    direct_aligned = len(direct_bytes) == len(raw_prefix)

    du_mis = first_mismatch_bytes(du_bytes, raw_prefix)
    bridge_mis = first_mismatch_bytes(bridge_bytes, raw_prefix)
    direct_mis = first_mismatch_bytes(direct_bytes[:len(raw_prefix)], raw_prefix)

    id_mis = None
    if direct_aligned:
        id_mis = first_mismatch_ids(direct_ids, bridge_ids)

    report = {
        "mode": "Nereid v0 target-vocab adapter",
        "claim": "DU-derived target IDs decode back to the same bytes",
        "not_claimed": "same ID segmentation as the original tokenizer",
        "model_bin_format": model_format,
        "du_run": str(du_run),
        "tik_remap": str(tik),
        "raw": str(raw),
        "gmap_width": gmap_width,
        "target_vocab_size": len(target_vocab),
        "target_vocab_max_token_bytes": greedy.max_len,
        "limit_bytes_requested": args.limit_bytes,
        "bytes_tested": len(raw_prefix),
        "du_positions_consumed": du_positions,
        "du_unique_ids_encoded": len(cache),
        "bridge_target_ids": len(bridge_ids),
        "direct_target_ids_covering_span": len(direct_ids),
        "direct_stream_byte_aligned_to_span": direct_aligned,
        "du_reconstruct_equals_raw_prefix": du_mis is None,
        "bridge_decode_equals_raw_prefix": bridge_mis is None,
        "direct_decode_prefix_equals_raw_prefix": direct_mis is None,
        "target_id_exact_when_aligned": (id_mis is None) if direct_aligned else None,
        "first_target_id_mismatch_when_aligned": id_mis,
        "first_du_raw_byte_mismatch": du_mis,
        "first_bridge_raw_byte_mismatch": bridge_mis,
        "first_direct_raw_byte_mismatch": direct_mis,
        "elapsed_s": round(time.time() - t0, 6),
    }

    if id_mis is not None:
        lo = max(0, id_mis - 16)
        hi = id_mis + 16
        report["direct_ids_around_mismatch"] = direct_ids[lo:hi]
        report["bridge_ids_around_mismatch"] = bridge_ids[lo:hi]

    (outdir / "report.json").write_text(
        json.dumps(report, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )

    with (outdir / "bridge.ids.u32").open("wb") as f:
        for x in bridge_ids:
            f.write(struct.pack("<I", int(x)))

    (outdir / "README.md").write_text(
        f"""# Nereid v0 — target-vocabulary adapter

```text
DU stream
 -> DU lexical bytes
 -> greedy segmentation using target model.bin token bytes
 -> target IDs
 -> decode through target model.bin
 -> original bytes
```

This receipt claims text-equivalent target vocabulary IDs.

It does not claim exact equality with the target tokenizer's preferred
segmentation.

```json
{json.dumps(report, indent=2, ensure_ascii=False)}
```
""",
        encoding="utf-8",
    )

    print(json.dumps(report, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
