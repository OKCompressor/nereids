#!/usr/bin/env python3
import argparse
import hashlib
import json
import shutil
import struct
from pathlib import Path

from mistral_common.tokens.tokenizers.mistral import MistralTokenizer

ap = argparse.ArgumentParser()
ap.add_argument("--raw", required=True, type=Path)
ap.add_argument("--model-bin", required=True, type=Path)
ap.add_argument("--out", required=True, type=Path)
ap.add_argument("--max-bytes", type=int, default=2_000_000)
args = ap.parse_args()

args.out.mkdir(parents=True, exist_ok=True)

raw = args.raw.read_bytes()
prefix = raw[:args.max_bytes]

# Shrink only if max-bytes cuts the last UTF-8 scalar.
while True:
    try:
        text = prefix.decode("utf-8")
        break
    except UnicodeDecodeError as e:
        if e.start < len(prefix) - 4:
            raise
        prefix = prefix[:e.start]

m = MistralTokenizer.v3(is_tekken=True)
t = m.instruct_tokenizer.tokenizer

ids = t.encode(text, bos=False, eos=False)

assert all(t.num_special_tokens <= i < t.n_words for i in ids)

rebuilt = b"".join(t.id_to_byte_piece(i) for i in ids)
assert rebuilt == prefix

shutil.copy2(args.model_bin, args.out / "model.bin")

with (args.out / "tokens.u32").open("wb") as f:
    for i in ids:
        f.write(struct.pack("<I", i))

(args.out / "raw-prefix.bin").write_bytes(prefix)

meta = {
    "schema": "nereids-mistral-tekken-canonical-stream-v1",
    "raw_source": str(args.raw),
    "prefix_bytes": len(prefix),
    "prefix_sha256": hashlib.sha256(prefix).hexdigest(),
    "canonical_native_ids": len(ids),
    "bytes_per_native_id": len(prefix) / len(ids),
    "bos": False,
    "eos": False,
    "exact_decode": rebuilt == prefix,
}

(args.out / "metadata.json").write_text(
    json.dumps(meta, indent=2) + "\n"
)

print(json.dumps(meta, indent=2))
