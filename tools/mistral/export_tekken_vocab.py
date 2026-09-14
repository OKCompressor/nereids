#!/usr/bin/env python3
import argparse
import base64
import hashlib
import json
import struct
from importlib.metadata import version
from pathlib import Path

from mistral_common.tokens.tokenizers.mistral import MistralTokenizer

ap = argparse.ArgumentParser()
ap.add_argument("out_dir", type=Path)
args = ap.parse_args()

out = args.out_dir
out.mkdir(parents=True, exist_ok=True)

m = MistralTokenizer.v3(is_tekken=True)
t = m.instruct_tokenizer.tokenizer

src = Path(t.file_path)
src_raw = src.read_bytes()
data = json.loads(src_raw)
cfg = data["config"]

vocab_size = int(cfg["default_vocab_size"])
num_special = int(cfg["default_num_special_tokens"])
mergeable_count = vocab_size - num_special

assert vocab_size == t.n_words
assert num_special == t.num_special_tokens

rows = data["vocab"][:mergeable_count]
assert len(rows) == mergeable_count

tokens = [b""] * num_special

for rank, row in enumerate(rows):
    assert row["rank"] == rank
    piece = base64.b64decode(row["token_bytes"])
    if rank < 256:
        assert piece == bytes([rank])
    tokens.append(piece)

assert len(tokens) == vocab_size

for native_id in range(num_special, vocab_size):
    assert tokens[native_id] == t.id_to_byte_piece(native_id)

model = out / "model.bin"

with model.open("wb") as f:
    f.write(struct.pack("<I", len(tokens)))
    for piece in tokens:
        f.write(struct.pack("<I", len(piece)))
        f.write(piece)

meta = {
    "schema": "nereids-target-vocab-metadata-v1",
    "family": "mistral",
    "model_family": "Mistral-Nemo",
    "tokenizer": "tekken",
    "tokenizer_version": str(t.version),
    "mistral_common_version": version("mistral-common"),
    "source_file": str(src),
    "source_sha256": hashlib.sha256(src_raw).hexdigest(),
    "native_vocab_size": vocab_size,
    "num_special_tokens": num_special,
    "special_id_range": [0, num_special - 1],
    "mergeable_id_range": [num_special, vocab_size - 1],
    "mergeable_entries": mergeable_count,
    "native_mergeable_id_offset": num_special,
    "model_bin_index_equals_native_model_id": True,
    "special_entries": "zero-length reserved placeholders",
    "model_bin_bytes": model.stat().st_size,
    "model_bin_sha256": hashlib.sha256(model.read_bytes()).hexdigest(),
}

(out / "metadata.json").write_text(
    json.dumps(meta, indent=2) + "\n"
)

print(json.dumps(meta, indent=2))
