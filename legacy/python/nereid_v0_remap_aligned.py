#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import struct
import time
from pathlib import Path


def load_model_bin(path: Path) -> list[bytes]:
    b = path.read_bytes()
    if len(b) < 4:
        raise ValueError("model.bin too small")

    k = struct.unpack_from("<I", b, 0)[0]
    off = 4
    vocab: list[bytes] = []

    for _ in range(k):
        if off + 4 > len(b):
            raise ValueError("truncated model.bin length")
        n = struct.unpack_from("<I", b, off)[0]
        off += 4
        if off + n > len(b):
            raise ValueError("truncated model.bin token")
        vocab.append(b[off:off+n])
        off += n

    if off != len(b):
        raise ValueError(f"trailing bytes in model.bin: {len(b)-off}")

    return vocab


def read_u32s(path: Path):
    b = path.read_bytes()
    if len(b) % 4:
        raise ValueError(f"bad u32 size: {path}")
    for (x,) in struct.iter_unpack("<I", b):
        yield x


def read_u16s(path: Path):
    b = path.read_bytes()
    if len(b) % 2:
        raise ValueError(f"bad u16 size: {path}")
    for (x,) in struct.iter_unpack("<H", b):
        yield x


def load_gmap(path: Path, width: int) -> list[int]:
    b = path.read_bytes()

    if width == 24:
        if len(b) % 3:
            raise ValueError(f"bad gmap24 size: {path}")
        return [
            b[i] | (b[i+1] << 8) | (b[i+2] << 16)
            for i in range(0, len(b), 3)
        ]

    if width == 32:
        if len(b) % 4:
            raise ValueError(f"bad gmap32 size: {path}")
        return [x[0] for x in struct.iter_unpack("<I", b)]

    raise ValueError(width)


def load_du_dict(path: Path) -> list[bytes]:
    # Matches the current line-oriented public artifact convention.
    # If this ever fails on escaped newline semantics, the receipt will say so
    # and this loader gets swapped to the exact redumb restore parser.
    out: list[bytes] = []
    with path.open("r", encoding="utf-8", newline="") as f:
        for line in f:
            if line.endswith("\n"):
                line = line[:-1]
            if line.endswith("\r"):
                line = line[:-1]
            out.append(line.encode("utf-8"))
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

    def encode(self, bs: bytes) -> list[int]:
        out: list[int] = []
        i = 0

        while i < len(bs):
            hit = None
            for tok, tid in self.by_first.get(bs[i], []):
                if bs.startswith(tok, i):
                    hit = (tok, tid)
                    break

            if hit is None:
                raise ValueError(
                    f"no vocab token at local byte offset {i}, byte=0x{bs[i]:02x}, sample={bs[max(0,i-16):i+32]!r}"
                )

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
    ap.add_argument("--min-bytes", type=int, default=1_000_000)
    ap.add_argument("--max-bytes", type=int, default=2_000_000)
    args = ap.parse_args()

    t0 = time.time()

    du_run = Path(args.du_run)
    tik = Path(args.tik_remap)
    raw = Path(args.raw)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    target_vocab = load_model_bin(tik / "model.bin")
    greedy = GreedyVocab(target_vocab)

    direct_boundaries: dict[int, int] = {0: 0}
    direct_ids_all: list[int] = []
    direct_bytes_parts: list[bytes] = []
    direct_len = 0

    for tid in read_u32s(tik / "tokens.u32"):
        if tid >= len(target_vocab):
            raise ValueError(f"direct target id out of range: {tid}")
        direct_ids_all.append(tid)
        bs = target_vocab[tid]
        direct_bytes_parts.append(bs)
        direct_len += len(bs)
        direct_boundaries[direct_len] = len(direct_ids_all)
        if direct_len >= args.max_bytes:
            break

    direct_bytes_all = b"".join(direct_bytes_parts)

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
    du_len = 0
    cache: dict[int, list[int]] = {}

    aligned_len = None
    direct_n_at_aligned = None

    for u16_path in sorted(local_dir.glob("*.u16")):
        stem = u16_path.name[:-4]
        gmap = load_gmap(gmap_dir / f"{stem}{gmap_suffix}", gmap_width)

        for lid in read_u16s(u16_path):
            gid = gmap[lid]
            bs = du_vocab[gid]
            du_parts.append(bs)
            du_len += len(bs)

            ids = cache.get(gid)
            if ids is None:
                ids = greedy.encode(bs)
                cache[gid] = ids
            bridge_ids.extend(ids)
            du_positions += 1

            if du_len >= args.min_bytes and du_len in direct_boundaries:
                aligned_len = du_len
                direct_n_at_aligned = direct_boundaries[du_len]
                break

            if du_len > args.max_bytes:
                break

        if aligned_len is not None or du_len > args.max_bytes:
            break

    if aligned_len is None:
        raise SystemExit(
            f"no shared DU/direct token boundary found in range {args.min_bytes}..{args.max_bytes}"
        )

    du_bytes = b"".join(du_parts)
    bridge_bytes = b"".join(target_vocab[i] for i in bridge_ids)
    raw_prefix = raw.read_bytes()[:aligned_len]
    direct_ids = direct_ids_all[:direct_n_at_aligned]
    direct_bytes = direct_bytes_all[:aligned_len]

    # Trim bridge to the aligned DU boundary. Since we stopped exactly at a DU
    # boundary, bridge_bytes should already be aligned_len bytes.
    if len(du_bytes) != aligned_len:
        raise AssertionError("internal DU length mismatch")
    if len(bridge_bytes) != aligned_len:
        raise AssertionError("bridge decode length mismatch")

    id_mis = first_mismatch_ids(direct_ids, bridge_ids)

    report = {
        "mode": "Nereid v0 cl100k_base dense-remap target-vocab adapter",
        "claim": "DU-derived remap IDs decode to same bytes at a shared DU/direct-token boundary",
        "not_claimed": "semantic equivalence or preferred tokenizer path equivalence when IDs differ",
        "du_run": str(du_run),
        "tik_remap": str(tik),
        "raw": str(raw),
        "encoding_from_meta": "cl100k_base",
        "target_vocab_size_used": len(target_vocab),
        "target_vocab_max_token_bytes": greedy.max_len,
        "direct_token_count_available_prefix": len(direct_ids_all),
        "shared_boundary_bytes": aligned_len,
        "du_positions_consumed": du_positions,
        "du_unique_ids_encoded": len(cache),
        "bridge_target_ids": len(bridge_ids),
        "direct_target_ids": len(direct_ids),
        "du_reconstruct_equals_raw_prefix": first_mismatch_bytes(du_bytes, raw_prefix) is None,
        "bridge_decode_equals_raw_prefix": first_mismatch_bytes(bridge_bytes, raw_prefix) is None,
        "direct_decode_equals_raw_prefix": first_mismatch_bytes(direct_bytes, raw_prefix) is None,
        "target_id_exact": id_mis is None,
        "first_target_id_mismatch": id_mis,
        "elapsed_s": round(time.time() - t0, 6),
    }

    if id_mis is not None:
        lo = max(0, id_mis - 16)
        hi = id_mis + 16
        report["direct_ids_around_mismatch"] = direct_ids[lo:hi]
        report["bridge_ids_around_mismatch"] = bridge_ids[lo:hi]
        report["direct_bytes_around_mismatch"] = b"".join(
            target_vocab[i] for i in direct_ids[lo:hi]
        ).decode("utf-8", errors="replace")
        report["bridge_bytes_around_mismatch"] = b"".join(
            target_vocab[i] for i in bridge_ids[lo:hi]
        ).decode("utf-8", errors="replace")

    (out / "report.json").write_text(
        json.dumps(report, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )

    with (out / "bridge.ids.u32").open("wb") as f:
        for x in bridge_ids:
            f.write(struct.pack("<I", int(x)))

    (out / "README.md").write_text(
        f"""# Nereid v0 — cl100k_base dense-remap adapter

```text
DU stream
 -> canonical DU lexical bytes
 -> greedy segmentation using bench-pre-v2 tiktoken_remap/model.bin
 -> dense remap IDs
 -> decode via model.bin
 -> original bytes
```

This receipt is aligned to a byte position that is both a DU token boundary
and a direct tiktoken-remap token boundary.

```json
{json.dumps(report, indent=2, ensure_ascii=False)}
```
""",
        encoding="utf-8",
    )

    print(json.dumps(report, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
