#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import struct
import time
from pathlib import Path

import tiktoken


def load_dict(path: Path) -> list[str]:
    # DUMBer merged.dict is line-oriented. Preserve spaces/tabs.
    # This intentionally matches the current public artifact format.
    out: list[str] = []
    with path.open("r", encoding="utf-8", newline="") as f:
        for line in f:
            if line.endswith("\n"):
                line = line[:-1]
            if line.endswith("\r"):
                line = line[:-1]
            out.append(line)
    return out


def read_u16s(path: Path):
    b = path.read_bytes()
    if len(b) % 2:
        raise ValueError(f"bad u16 size: {path}")
    for (x,) in struct.iter_unpack("<H", b):
        yield x


def load_gmap(path: Path, width: int) -> list[int]:
    b = path.read_bytes()
    step = 3 if width == 24 else 4
    if len(b) % step:
        raise ValueError(f"bad gmap{width} size: {path}")

    out: list[int] = []
    if width == 24:
        for i in range(0, len(b), 3):
            out.append(b[i] | (b[i + 1] << 8) | (b[i + 2] << 16))
    else:
        for (x,) in struct.iter_unpack("<I", b):
            out.append(x)
    return out


def enc_ids(enc, s: str) -> list[int]:
    if hasattr(enc, "encode_ordinary"):
        return enc.encode_ordinary(s)
    return enc.encode(s, allowed_special=set(), disallowed_special=())


def first_mismatch(a: list[int], b: list[int]) -> int | None:
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
    ap.add_argument("--out", required=True)
    ap.add_argument("--encoding", default="gpt2")
    ap.add_argument("--limit-bytes", type=int, default=1_000_000)
    args = ap.parse_args()

    t0 = time.time()

    du_run = Path(args.du_run)
    raw_path = Path(args.raw)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    merged = du_run / "merged.dict"
    local_dir = du_run / "local_u16"

    if (du_run / "gmap24").is_dir():
        gmap_dir = du_run / "gmap24"
        width = 24
        suffix = ".gmap24"
    elif (du_run / "gmap32").is_dir():
        gmap_dir = du_run / "gmap32"
        width = 32
        suffix = ".gmap32"
    else:
        raise SystemExit("missing gmap24/gmap32")

    vocab = load_dict(merged)
    enc = tiktoken.get_encoding(args.encoding)

    raw_bytes = raw_path.read_bytes()

    bridge_ids: list[int] = []
    du_text_parts: list[str] = []
    du_ids_seen = 0
    cache: dict[int, list[int]] = {}

    byte_len = 0

    chunks = sorted(local_dir.glob("*.u16"))
    if not chunks:
        raise SystemExit("no local_u16 chunks found")

    for u16_path in chunks:
        stem = u16_path.stem
        gmap_path = gmap_dir / f"{stem}{suffix}"
        if not gmap_path.exists():
            # tolerate names like chunk_000.u16 -> chunk_000.gmap24
            gmap_path = gmap_dir / f"{stem.replace('.u16', '')}{suffix}"
        if not gmap_path.exists():
            raise FileNotFoundError(gmap_path)

        gmap = load_gmap(gmap_path, width)

        for local_id in read_u16s(u16_path):
            gid = gmap[local_id]
            tok = vocab[gid]

            du_text_parts.append(tok)
            byte_len += len(tok.encode("utf-8"))

            ids = cache.get(gid)
            if ids is None:
                ids = enc_ids(enc, tok)
                cache[gid] = ids

            bridge_ids.extend(ids)
            du_ids_seen += 1

            if byte_len >= args.limit_bytes:
                break

        if byte_len >= args.limit_bytes:
            break

    du_text = "".join(du_text_parts)
    du_bytes = du_text.encode("utf-8")
    raw_prefix = raw_bytes[:len(du_bytes)]
    raw_text = raw_prefix.decode("utf-8")

    direct_ids = enc_ids(enc, raw_text)
    decoded_bridge = enc.decode(bridge_ids)

    text_equiv = decoded_bridge.encode("utf-8") == raw_prefix
    du_reconstruct_equiv = du_bytes == raw_prefix

    mid = first_mismatch(direct_ids, bridge_ids)

    report = {
        "mode": "Nereid v0 text-equivalent bridge",
        "encoding": args.encoding,
        "du_run": str(du_run),
        "raw": str(raw_path),
        "gmap_width": width,
        "limit_bytes_requested": args.limit_bytes,
        "bytes_tested": len(raw_prefix),
        "du_positions_consumed": du_ids_seen,
        "du_unique_cache_entries": len(cache),
        "direct_target_ids": len(direct_ids),
        "bridge_target_ids": len(bridge_ids),
        "du_reconstruct_equals_raw_prefix": du_reconstruct_equiv,
        "bridge_decode_equals_raw_prefix": text_equiv,
        "target_id_exact": mid is None,
        "first_target_id_mismatch": mid,
        "elapsed_s": round(time.time() - t0, 6),
    }

    if mid is not None:
        lo = max(0, mid - 16)
        hi = mid + 16
        report["direct_ids_around_mismatch"] = direct_ids[lo:hi]
        report["bridge_ids_around_mismatch"] = bridge_ids[lo:hi]

    (out / "report.json").write_text(
        json.dumps(report, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )

    with (out / "bridge.ids.u32").open("wb") as f:
        for x in bridge_ids:
            f.write(struct.pack("<I", int(x)))

    (out / "README.md").write_text(
        f"""# Nereid v0 — DU to {args.encoding} text-equivalent bridge

This receipt tests the simplest bridge:

```text
DU stream
 -> DU lexical strings
 -> {args.encoding} IDs per DU token
 -> target ID stream
 -> target decode
```

It proves whether the generated target-ID stream is valid and decodes to the
same text prefix.

It separately reports whether the target IDs are identical to direct
tokenization of the same raw text prefix.

## Result

```json
{json.dumps(report, indent=2, ensure_ascii=False)}
```

## Interpretation

- `bridge_decode_equals_raw_prefix=true` means the bridge produced a valid
  target-token stream that decodes to the original bytes tested.
- `target_id_exact=true` would mean model-tokenizer equivalence.
- If `target_id_exact=false`, the bridge is text-equivalent but not necessarily
  model-equivalent for a pretrained model.

This is intentionally a v0 control receipt.
""",
        encoding="utf-8",
    )

    print(json.dumps(report, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
