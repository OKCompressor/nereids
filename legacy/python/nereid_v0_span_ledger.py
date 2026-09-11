#!/usr/bin/env python3
from __future__ import annotations

import argparse
import bisect
import json
import struct
import time
from pathlib import Path


def load_model_bin(path: Path) -> list[bytes]:
    b = path.read_bytes()
    k = struct.unpack_from("<I", b, 0)[0]
    off = 4
    vocab: list[bytes] = []
    for _ in range(k):
        n = struct.unpack_from("<I", b, off)[0]
        off += 4
        vocab.append(b[off:off+n])
        off += n
    if off != len(b):
        raise ValueError(f"model.bin trailing bytes: {len(b)-off}")
    return vocab


def read_u32s(path: Path):
    b = path.read_bytes()
    if len(b) % 4:
        raise ValueError(path)
    for (x,) in struct.iter_unpack("<I", b):
        yield x


def read_u16s(path: Path):
    b = path.read_bytes()
    if len(b) % 2:
        raise ValueError(path)
    for (x,) in struct.iter_unpack("<H", b):
        yield x


def load_gmap(path: Path, width: int) -> list[int]:
    b = path.read_bytes()
    if width == 24:
        return [
            b[i] | (b[i + 1] << 8) | (b[i + 2] << 16)
            for i in range(0, len(b), 3)
        ]
    return [x[0] for x in struct.iter_unpack("<I", b)]


def load_du_dict(path: Path) -> list[bytes]:
    out: list[bytes] = []
    with path.open("r", encoding="utf-8", newline="") as f:
        for line in f:
            if line.endswith("\n"):
                line = line[:-1]
            if line.endswith("\r"):
                line = line[:-1]
            out.append(line.encode("utf-8"))
    return out


def first_mismatch(a: bytes, b: bytes) -> int | None:
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
    ap.add_argument("--max-bytes", type=int, default=2_000_000)
    ap.add_argument("--sample", type=int, default=80)
    args = ap.parse_args()

    t0 = time.time()

    du_run = Path(args.du_run)
    tik = Path(args.tik_remap)
    raw = Path(args.raw)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    target_vocab = load_model_bin(tik / "model.bin")
    target_ids: list[int] = []
    target_bounds: list[int] = [0]
    target_parts: list[bytes] = []

    n = 0
    for tid in read_u32s(tik / "tokens.u32"):
        bs = target_vocab[tid]
        target_ids.append(tid)
        target_parts.append(bs)
        n += len(bs)
        target_bounds.append(n)
        if n >= args.max_bytes:
            break

    target_bytes = b"".join(target_parts)

    du_vocab = load_du_dict(du_run / "merged.dict")
    local_dir = du_run / "local_u16"

    if (du_run / "gmap24").is_dir():
        gmap_dir = du_run / "gmap24"
        gmap_width = 24
        gmap_suffix = ".gmap24"
    else:
        gmap_dir = du_run / "gmap32"
        gmap_width = 32
        gmap_suffix = ".gmap32"

    du_ids: list[int] = []
    du_bounds: list[int] = [0]
    du_parts: list[bytes] = []
    du_n = 0

    for u16_path in sorted(local_dir.glob("*.u16")):
        stem = u16_path.name[:-4]
        gmap = load_gmap(gmap_dir / f"{stem}{gmap_suffix}", gmap_width)

        for lid in read_u16s(u16_path):
            gid = gmap[lid]
            bs = du_vocab[gid]
            du_ids.append(gid)
            du_parts.append(bs)
            du_n += len(bs)
            du_bounds.append(du_n)
            if du_n >= args.max_bytes:
                break

        if du_n >= args.max_bytes:
            break

    du_bytes = b"".join(du_parts)

    limit = min(len(target_bytes), len(du_bytes), args.max_bytes)
    raw_bytes = raw.read_bytes()[:limit]
    target_bytes = target_bytes[:limit]
    du_bytes = du_bytes[:limit]

    # Only count target tokens fully inside `limit`.
    target_token_count = bisect.bisect_right(target_bounds, limit) - 1
    target_ids = target_ids[:target_token_count]
    target_bounds = target_bounds[:target_token_count + 1]

    exact_boundary = 0
    crosses_du = 0
    starts_inside_du = 0
    ends_inside_du = 0
    max_du_span = 0
    samples = []

    du_boundary_set = set(du_bounds)

    for ti, tid in enumerate(target_ids):
        a = target_bounds[ti]
        b = target_bounds[ti + 1]

        du_start = bisect.bisect_right(du_bounds, a) - 1
        du_end = bisect.bisect_left(du_bounds, b)

        # DU tokens covering target byte span are du_start .. du_end-1.
        span_len = max(0, du_end - du_start)
        max_du_span = max(max_du_span, span_len)

        start_boundary = a in du_boundary_set
        end_boundary = b in du_boundary_set

        if start_boundary and end_boundary:
            exact_boundary += 1
        else:
            if not start_boundary:
                starts_inside_du += 1
            if not end_boundary:
                ends_inside_du += 1

        if span_len != 1 or not (start_boundary and end_boundary):
            crosses_du += 1
            if len(samples) < args.sample:
                around_du = []
                for j in range(max(0, du_start - 3), min(len(du_ids), du_end + 3)):
                    bs = du_parts[j]
                    around_du.append({
                        "du_pos": j,
                        "du_id": du_ids[j],
                        "byte_start": du_bounds[j],
                        "byte_end": du_bounds[j + 1],
                        "repr": repr(bs[:80]),
                    })

                samples.append({
                    "target_pos": ti,
                    "target_id": tid,
                    "target_byte_start": a,
                    "target_byte_end": b,
                    "target_bytes_repr": repr(target_vocab[tid][:120]),
                    "du_start_pos": du_start,
                    "du_end_pos_exclusive": du_end,
                    "du_span_len": span_len,
                    "target_starts_on_du_boundary": start_boundary,
                    "target_ends_on_du_boundary": end_boundary,
                    "du_context": around_du,
                })

    report = {
        "mode": "Nereid v0 span ledger",
        "claim": "align existing direct target-ID stream against canonical DU stream by byte spans",
        "not_claimed": "new-corpus generation without target tokenizer; this is the alignment/training ledger",
        "target": "bench-pre-v2 tiktoken_remap dense cl100k_base used-vocab",
        "du_run": str(du_run),
        "tik_remap": str(tik),
        "raw": str(raw),
        "gmap_width": gmap_width,
        "bytes_tested": limit,
        "target_vocab_size_used": len(target_vocab),
        "target_tokens_tested": target_token_count,
        "du_tokens_covering_prefix": bisect.bisect_right(du_bounds, limit) - 1,
        "target_decode_equals_raw_prefix": first_mismatch(target_bytes, raw_bytes) is None,
        "du_decode_equals_raw_prefix": first_mismatch(du_bytes, raw_bytes) is None,
        "target_decode_first_mismatch": first_mismatch(target_bytes, raw_bytes),
        "du_decode_first_mismatch": first_mismatch(du_bytes, raw_bytes),
        "target_tokens_exactly_on_du_boundaries": exact_boundary,
        "target_tokens_cross_or_cut_du_tokens": crosses_du,
        "target_tokens_start_inside_du_token": starts_inside_du,
        "target_tokens_end_inside_du_token": ends_inside_du,
        "max_du_tokens_covered_by_one_target_token": max_du_span,
        "fraction_exact_boundary": round(exact_boundary / target_token_count, 6) if target_token_count else None,
        "elapsed_s": round(time.time() - t0, 6),
    }

    (out / "report.json").write_text(
        json.dumps(report, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )

    with (out / "span_samples.jsonl").open("w", encoding="utf-8") as f:
        for x in samples:
            f.write(json.dumps(x, ensure_ascii=False) + "\n")

    (out / "README.md").write_text(
        f"""# Nereid v0 — span ledger

This receipt aligns the existing direct target stream against the canonical DU
stream by byte spans.

```text
direct cl100k-remap token ID stream
        ↕ byte spans
canonical DU stream
```

It is the correct first bridge artifact when the target remap vocabulary is a
used-vocab subset and is not closed under arbitrary DU token boundaries.

```json
{json.dumps(report, indent=2, ensure_ascii=False)}
```

See `span_samples.jsonl` for target tokens that cross or cut DU token
boundaries.
""",
        encoding="utf-8",
    )

    print(json.dumps(report, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
