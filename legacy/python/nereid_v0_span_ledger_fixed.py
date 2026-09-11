#!/usr/bin/env python3
from __future__ import annotations

import argparse, base64, bisect, json, re, struct, time
from pathlib import Path


def decode_du_lexeme(s: str) -> bytes:
    def b64_restore(m: re.Match[str]) -> str:
        enc = m.group(1)
        pad = "=" * (-len(enc) % 4)
        return base64.b64decode((enc + pad).encode("ascii")).decode("utf-8")

    s = re.sub("\u0004([A-Za-z0-9+/=]+)\u0005", b64_restore, s)
    s = s.replace("\u0003", "\n")
    s = s.replace("\u0001", "\t")
    s = re.sub("\u0002([0-9]+)\u0002", lambda m: " " * int(m.group(1)), s)
    return s.encode("utf-8")


def load_du_dict(path: Path) -> list[bytes]:
    out = []
    with path.open("r", encoding="utf-8", newline="") as f:
        for line in f:
            if line.endswith("\n"):
                line = line[:-1]
            if line.endswith("\r"):
                line = line[:-1]
            out.append(decode_du_lexeme(line))
    return out


def load_model_bin(path: Path) -> list[bytes]:
    b = path.read_bytes()
    k = struct.unpack_from("<I", b, 0)[0]
    off = 4
    vocab = []
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
    for (x,) in struct.iter_unpack("<I", b):
        yield x


def read_u16s(path: Path):
    b = path.read_bytes()
    for (x,) in struct.iter_unpack("<H", b):
        yield x


def load_gmap(path: Path, width: int) -> list[int]:
    b = path.read_bytes()
    if width == 24:
        return [b[i] | (b[i+1] << 8) | (b[i+2] << 16) for i in range(0, len(b), 3)]
    return [x[0] for x in struct.iter_unpack("<I", b)]


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
    args = ap.parse_args()

    t0 = time.time()
    du_run = Path(args.du_run)
    raw_path = Path(args.raw)
    tik = Path(args.tik_remap)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    target_vocab = load_model_bin(tik / "model.bin")

    target_ids = []
    target_bounds = [0]
    target_parts = []
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
        gmap_dir, width, suffix = du_run / "gmap24", 24, ".gmap24"
    else:
        gmap_dir, width, suffix = du_run / "gmap32", 32, ".gmap32"

    du_ids = []
    du_bounds = [0]
    du_parts = []
    dn = 0
    for u16_path in sorted(local_dir.glob("*.u16")):
        gmap = load_gmap(gmap_dir / f"{u16_path.name[:-4]}{suffix}", width)
        for lid in read_u16s(u16_path):
            gid = gmap[lid]
            bs = du_vocab[gid]
            du_ids.append(gid)
            du_parts.append(bs)
            dn += len(bs)
            du_bounds.append(dn)
            if dn >= args.max_bytes:
                break
        if dn >= args.max_bytes:
            break

    du_bytes = b"".join(du_parts)
    limit = min(len(target_bytes), len(du_bytes), args.max_bytes)
    raw = raw_path.read_bytes()[:limit]
    target_bytes = target_bytes[:limit]
    du_bytes = du_bytes[:limit]

    target_count = bisect.bisect_right(target_bounds, limit) - 1
    target_ids = target_ids[:target_count]
    target_bounds = target_bounds[:target_count+1]

    du_boundary_set = set(du_bounds)
    exact = cut_or_cross = start_inside = end_inside = 0
    max_du_span = 0
    samples = []

    for ti, tid in enumerate(target_ids):
        a, b = target_bounds[ti], target_bounds[ti+1]
        ds = bisect.bisect_right(du_bounds, a) - 1
        de = bisect.bisect_left(du_bounds, b)
        span = max(0, de - ds)
        max_du_span = max(max_du_span, span)

        sb = a in du_boundary_set
        eb = b in du_boundary_set

        if sb and eb:
            exact += 1
        else:
            cut_or_cross += 1
            if not sb:
                start_inside += 1
            if not eb:
                end_inside += 1
            if len(samples) < 80:
                samples.append({
                    "target_pos": ti,
                    "target_id_dense": tid,
                    "target_byte_start": a,
                    "target_byte_end": b,
                    "target_bytes_repr": repr(target_vocab[tid]),
                    "du_start_pos": ds,
                    "du_end_pos_exclusive": de,
                    "du_span_len": span,
                    "target_starts_on_du_boundary": sb,
                    "target_ends_on_du_boundary": eb,
                    "du_context": [
                        {
                            "du_pos": j,
                            "du_id": du_ids[j],
                            "byte_start": du_bounds[j],
                            "byte_end": du_bounds[j+1],
                            "repr": repr(du_parts[j][:100]),
                        }
                        for j in range(max(0, ds-3), min(len(du_ids), de+3))
                    ],
                })

    tm = first_mismatch(target_bytes, raw)
    dm = first_mismatch(du_bytes, raw)

    report = {
        "mode": "Nereid v0 fixed span ledger",
        "target": "bench-pre-v2 dense cl100k_base used-vocab remap",
        "claim": "align direct target-remap stream and decoded DU stream by raw byte spans",
        "not_claimed": "official model IDs or full target tokenizer generation",
        "bytes_tested": limit,
        "target_vocab_size_used": len(target_vocab),
        "target_tokens_tested": target_count,
        "du_tokens_covering_prefix": bisect.bisect_right(du_bounds, limit) - 1,
        "target_decode_equals_raw_prefix": tm is None,
        "du_decode_equals_raw_prefix": dm is None,
        "target_decode_first_mismatch": tm,
        "du_decode_first_mismatch": dm,
        "target_tokens_exactly_on_du_boundaries": exact,
        "target_tokens_cross_or_cut_du_tokens": cut_or_cross,
        "target_tokens_start_inside_du_token": start_inside,
        "target_tokens_end_inside_du_token": end_inside,
        "max_du_tokens_covered_by_one_target_token": max_du_span,
        "fraction_exact_boundary": round(exact / target_count, 6) if target_count else None,
        "elapsed_s": round(time.time() - t0, 6),
    }

    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    with (out / "span_samples.jsonl").open("w", encoding="utf-8") as f:
        for s in samples:
            f.write(json.dumps(s, ensure_ascii=False) + "\n")

    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
