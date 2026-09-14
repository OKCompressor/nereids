#!/usr/bin/env python3
import struct
import sys
from pathlib import Path

p = Path(sys.argv[1])
raw = p.read_bytes()

count = struct.unpack_from("<I", raw, 0)[0]
off = 4
lens = []

for _ in range(count):
    n = struct.unpack_from("<I", raw, off)[0]
    off += 4
    lens.append(n)
    off += n

assert off == len(raw)

nonempty = sorted(x for x in lens if x)

def pct(p):
    i = min(len(nonempty) - 1, int((len(nonempty) - 1) * p))
    return nonempty[i]

print("entries =", count)
print("empty_entries =", count - len(nonempty))
print("nonempty_entries =", len(nonempty))
print("token_payload_bytes =", sum(lens))
print("avg_nonempty_token_bytes =", sum(nonempty) / len(nonempty))
print("p50 =", pct(.50))
print("p90 =", pct(.90))
print("p99 =", pct(.99))
print("max =", nonempty[-1])
