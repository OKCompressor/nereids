import base64
import statistics
import sys
from pathlib import Path

path = Path(sys.argv[1])
raw = path.read_text(encoding="utf-8")

lines = raw.split("\n")
if raw.endswith("\n"):
    lines.pop()

def decode_du(s):
    out = []
    i = 0

    while i < len(s):
        c = s[i]

        if c == "\x04":
            j = s.index("\x05", i + 1)
            enc = s[i+1:j]
            enc += "=" * ((-len(enc)) % 4)
            b = base64.b64decode(enc)
            out.append(b.decode("utf-8"))
            i = j + 1

        elif c == "\x03":
            out.append("\n")
            i += 1

        elif c == "\x01":
            out.append("\t")
            i += 1

        elif c == "\x02":
            j = s.index("\x02", i + 1)
            n = int(s[i+1:j])
            out.append(" " * n)
            i = j + 1

        elif c == "\x00":
            raise ValueError("NUL in DU lexeme")

        else:
            out.append(c)
            i += 1

    return "".join(out).encode("utf-8")

vocab = [decode_du(x.rstrip("\r")) for x in lines]
lens = sorted(map(len, vocab))

def pct(p):
    return lens[min(len(lens)-1, int((len(lens)-1)*p))]

print("entries =", len(vocab))
print("serialized_file_bytes =", path.stat().st_size)
print("decoded_dictionary_bytes =", sum(lens))
print("decoded_avg_bytes =", sum(lens) / len(lens))
print("decoded_min =", lens[0])
print("decoded_p50 =", pct(.50))
print("decoded_p90 =", pct(.90))
print("decoded_p99 =", pct(.99))
print("decoded_max =", lens[-1])

print("\n=== printable ASCII peeks ===")
shown = 0
for gid, b in enumerate(vocab):
    if b and all(32 <= x < 127 for x in b):
        print(f"gid={gid:6d} len={len(b):3d}  {b!r}")
        shown += 1
        if shown == 20:
            break

print("\n=== longest lexemes ===")
for gid, b in sorted(enumerate(vocab), key=lambda x: len(x[1]), reverse=True)[:15]:
    preview = repr(b[:100])
    print(f"gid={gid:6d} len={len(b):5d}  {preview}")
