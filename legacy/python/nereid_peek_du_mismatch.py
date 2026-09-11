#!/usr/bin/env python3
from __future__ import annotations

import base64
import re
import struct
from pathlib import Path


DU_RUN = Path("/mnt/data_linux/IT/OKC-releases/_work/du-merger-poc-enwik8-20260902-211853")
RAW = Path("/mnt/data_linux/IT/OKC/enwiks/enwik8")


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


def load_du_dict(path: Path, decode: bool) -> list[bytes]:
    out = []
    with path.open("r", encoding="utf-8", newline="") as f:
        for line in f:
            if line.endswith("\n"):
                line = line[:-1]
            if line.endswith("\r"):
                line = line[:-1]
            out.append(decode_du_lexeme(line) if decode else line.encode("utf-8"))
    return out


def read_u16s(path: Path):
    b = path.read_bytes()
    for (x,) in struct.iter_unpack("<H", b):
        yield x


def load_gmap24(path: Path) -> list[int]:
    b = path.read_bytes()
    return [b[i] | (b[i+1] << 8) | (b[i+2] << 16) for i in range(0, len(b), 3)]


def first_mis(a: bytes, b: bytes):
    for i in range(min(len(a), len(b))):
        if a[i] != b[i]:
            return i
    if len(a) != len(b):
        return min(len(a), len(b))
    return None


def build_prefix(decode: bool, limit: int = 512):
    vocab = load_du_dict(DU_RUN / "merged.dict", decode=decode)
    u16 = sorted((DU_RUN / "local_u16").glob("*.u16"))[0]
    gmap = load_gmap24((DU_RUN / "gmap24") / (u16.name[:-4] + ".gmap24"))

    parts = []
    trace = []
    pos = 0
    for du_pos, lid in enumerate(read_u16s(u16)):
        gid = gmap[lid]
        bs = vocab[gid]
        parts.append(bs)
        trace.append((du_pos, lid, gid, pos, pos + len(bs), repr(bs)))
        pos += len(bs)
        if pos >= limit:
            break
    return b"".join(parts), trace


raw = RAW.read_bytes()[:512]

for decode in (False, True):
    got, trace = build_prefix(decode=decode)
    m = first_mis(got[:512], raw)
    print()
    print("decode_magic", decode)
    print("first_mismatch", m)
    if m is not None:
        print("raw around", m, repr(raw[max(0,m-80):m+120]))
        print("got around", m, repr(got[max(0,m-80):m+120]))
        print("tokens around mismatch")
        for row in trace:
            if row[3] <= m + 40 and row[4] >= m - 40:
                print(row)
