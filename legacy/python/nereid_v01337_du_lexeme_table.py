#!/usr/bin/env python3
from __future__ import annotations

import argparse, base64, json, re, struct, time
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


def read_u16s(path: Path):
    b = path.read_bytes()
    for (x,) in struct.iter_unpack("<H", b):
        yield x


def load_gmap(path: Path, width: int) -> list[int]:
    b = path.read_bytes()
    if width == 24:
        return [b[i] | (b[i+1] << 8) | (b[i+2] << 16) for i in range(0, len(b), 3)]
    return [x[0] for x in struct.iter_unpack("<I", b)]


class GreedyTarget:
    def __init__(self, vocab: list[bytes]):
        self.vocab = vocab
        self.by_first = {}
        self.max_len = 0
        for tid, bs in enumerate(vocab):
            if not bs:
                continue
            self.by_first.setdefault(bs[0], []).append((bs, tid))
            self.max_len = max(self.max_len, len(bs))
        for k in self.by_first:
            self.by_first[k].sort(key=lambda x: len(x[0]), reverse=True)

    def encode_bytes(self, bs: bytes) -> list[int]:
        out = []
        i = 0
        while i < len(bs):
            hit = None
            for tok, tid in self.by_first.get(bs[i], []):
                if bs.startswith(tok, i):
                    hit = (tok, tid)
                    break
            if hit is None:
                raise ValueError(f"no target token at byte {i}: {bs[max(0,i-20):i+40]!r}")
            tok, tid = hit
            out.append(tid)
            i += len(tok)
        return out

    def decode_ids(self, ids: list[int]) -> bytes:
        return b"".join(self.vocab[i] for i in ids)


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

    du_vocab = load_du_dict(du_run / "merged.dict")
    target_vocab = load_model_bin(tik / "model.bin")
    target = GreedyTarget(target_vocab)

    table: list[list[int] | None] = []
    fail_samples = []
    ok_types = 0
    fail_types = 0
    bytes_ok_types = 0
    bytes_fail_types = 0

    for gid, bs in enumerate(du_vocab):
        try:
            ids = target.encode_bytes(bs)
            table.append(ids)
            ok_types += 1
            bytes_ok_types += len(bs)
        except Exception as e:
            table.append(None)
            fail_types += 1
            bytes_fail_types += len(bs)
            if len(fail_samples) < 50:
                fail_samples.append({
                    "du_id": gid,
                    "bytes_len": len(bs),
                    "bytes_repr": repr(bs[:160]),
                    "error": str(e),
                })

    if (du_run / "gmap24").is_dir():
        gmap_dir, width, suffix = du_run / "gmap24", 24, ".gmap24"
    else:
        gmap_dir, width, suffix = du_run / "gmap32", 32, ".gmap32"

    local_dir = du_run / "local_u16"

    bridge_ids = []
    du_parts = []
    du_positions = 0
    byte_len = 0
    position_fail = None

    for u16_path in sorted(local_dir.glob("*.u16")):
        gmap = load_gmap(gmap_dir / f"{u16_path.name[:-4]}{suffix}", width)

        for lid in read_u16s(u16_path):
            gid = gmap[lid]
            bs = du_vocab[gid]
            ids = table[gid]

            if ids is None:
                position_fail = {
                    "du_pos": du_positions,
                    "du_id": gid,
                    "bytes_repr": repr(bs[:160]),
                }
                break

            du_parts.append(bs)
            bridge_ids.extend(ids)
            du_positions += 1
            byte_len += len(bs)

            if byte_len >= args.max_bytes:
                break

        if position_fail is not None or byte_len >= args.max_bytes:
            break

    du_bytes = b"".join(du_parts)
    bridge_bytes = target.decode_ids(bridge_ids)
    raw_prefix = raw_path.read_bytes()[:len(du_bytes)]

    du_mis = first_mismatch(du_bytes, raw_prefix)
    bridge_mis = first_mismatch(bridge_bytes, raw_prefix)

    table_path = out / "du_to_target_dense_ids.jsonl"
    with table_path.open("w", encoding="utf-8") as f:
        for gid, ids in enumerate(table):
            if ids is not None:
                f.write(json.dumps({"du_id": gid, "target_ids_dense": ids}) + "\n")

    with (out / "bridge.ids.u32").open("wb") as f:
        for x in bridge_ids:
            f.write(struct.pack("<I", int(x)))

    report = {
        "mode": "Nereid v0.1337 DU lexeme table",
        "target": "bench-pre-v2 dense cl100k_base used-vocab remap",
        "claim": "precompute DU_ID -> target ID sequence, then expand DU stream",
        "not_claimed": "official cl100k IDs or direct tokenizer path equivalence",
        "du_vocab_size": len(du_vocab),
        "target_vocab_size_used": len(target_vocab),
        "target_max_token_bytes": target.max_len,
        "du_types_segmentable": ok_types,
        "du_types_not_segmentable": fail_types,
        "du_type_segmentable_fraction": round(ok_types / len(du_vocab), 6) if du_vocab else None,
        "du_lexical_bytes_segmentable": bytes_ok_types,
        "du_lexical_bytes_not_segmentable": bytes_fail_types,
        "prefix_bytes_requested": args.max_bytes,
        "prefix_bytes_emitted": len(raw_prefix),
        "du_positions_emitted": du_positions,
        "bridge_target_ids_emitted": len(bridge_ids),
        "position_fail": position_fail,
        "du_decode_equals_raw_prefix": du_mis is None,
        "bridge_decode_equals_raw_prefix": bridge_mis is None,
        "du_decode_first_mismatch": du_mis,
        "bridge_decode_first_mismatch": bridge_mis,
        "elapsed_s": round(time.time() - t0, 6),
    }

    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    with (out / "fail_samples.json").open("w", encoding="utf-8") as f:
        json.dump(fail_samples, f, indent=2, ensure_ascii=False)

    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
