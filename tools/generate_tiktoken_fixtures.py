#!/usr/bin/env python3
"""Generate or verify public tiktoken native-token JSONL fixtures.

The generated offsets are byte offsets into the raw input. This tool is
deliberately dependency-gated: without tiktoken it reports BLOCKED_TIKTOKEN
and never creates fixture files.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import sys
from pathlib import Path


ENCODINGS = ("gpt2", "cl100k_base")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_tiktoken():
    try:
        import tiktoken
    except ImportError as exc:
        print(f"BLOCKED_TIKTOKEN: {exc}", file=sys.stderr)
        return None, None

    version = getattr(tiktoken, "__version__", None)
    if version is None:
        version = importlib.metadata.version("tiktoken")
    return tiktoken, version


def records_for(encoding, raw: bytes) -> list[dict[str, object]]:
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError(
            "the public tiktoken fixture input must be valid UTF-8 "
            f"(decode failed at byte {exc.start})"
        ) from exc

    records = []
    offset = 0
    for token_id in encoding.encode_ordinary(text):
        token_bytes = encoding.decode_single_token_bytes(token_id)
        end = offset + len(token_bytes)
        records.append(
            {
                "id": int(token_id),
                "byte_start": offset,
                "byte_end": end,
                "bytes_hex": token_bytes.hex(),
            }
        )
        offset = end

    reconstructed = b"".join(bytes.fromhex(record["bytes_hex"]) for record in records)
    if reconstructed != raw:
        raise ValueError(
            f"{encoding.name} token bytes do not reconstruct the input "
            f"(first mismatch: {first_mismatch(raw, reconstructed)})"
        )
    return records


def first_mismatch(expected: bytes, actual: bytes) -> int | None:
    for index, (left, right) in enumerate(zip(expected, actual)):
        if left != right:
            return index
    if len(expected) != len(actual):
        return min(len(expected), len(actual))
    return None


def fixture_bytes(records: list[dict[str, object]]) -> bytes:
    return b"".join(
        (json.dumps(record, separators=(",", ":"), ensure_ascii=True) + "\n").encode(
            "ascii"
        )
        for record in records
    )


def provenance(
    tokenizer_name: str,
    version: str,
    raw_path: Path,
    raw: bytes,
    records: list[dict[str, object]],
    fixture: bytes,
) -> dict[str, object]:
    return {
        "tokenizer_name": tokenizer_name,
        "tokenizer_provenance": f"tiktoken=={version}",
        "tiktoken_version": version,
        "input_file": str(raw_path),
        "input_byte_length": len(raw),
        "input_sha256": sha256(raw),
        "token_count": len(records),
        "fixture_sha256": sha256(fixture),
    }


def generated_files(
    output_dir: Path, raw_path: Path, raw: bytes, tiktoken, version: str
) -> dict[Path, bytes]:
    files: dict[Path, bytes] = {}
    for name in ENCODINGS:
        encoding = tiktoken.get_encoding(name)
        records = records_for(encoding, raw)
        fixture = fixture_bytes(records)
        fixture_path = output_dir / f"{name}.jsonl"
        provenance_path = output_dir / f"{name}.provenance.json"
        metadata = provenance(name, version, raw_path, raw, records, fixture)
        files[fixture_path] = fixture
        files[provenance_path] = (
            (json.dumps(metadata, indent=2, sort_keys=True) + "\n").encode("utf-8")
        )
    return files


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--raw",
        type=Path,
        default=Path("fixtures/multilingual.txt"),
        help="raw UTF-8 fixture input (default: fixtures/multilingual.txt)",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("fixtures/tiktoken"),
        help="fixture output directory (default: fixtures/tiktoken)",
    )
    parser.add_argument(
        "--verify",
        action="store_true",
        help="verify checked-in JSONL and provenance instead of writing files",
    )
    args = parser.parse_args()

    tiktoken, version = load_tiktoken()
    if tiktoken is None:
        return 2

    raw = args.raw.read_bytes()
    expected = generated_files(args.output_dir, args.raw, raw, tiktoken, version)
    if args.verify:
        failed = False
        for path, content in expected.items():
            if not path.is_file():
                print(f"MISSING: {path}", file=sys.stderr)
                failed = True
                continue
            actual = path.read_bytes()
            if actual != content:
                print(f"MISMATCH: {path}", file=sys.stderr)
                failed = True
        if failed:
            return 1
        print(f"FIXTURE_VERIFIED tiktoken=={version}")
        for name in ENCODINGS:
            records = json.loads(
                b"[" + b",".join(
                    line for line in (args.output_dir / f"{name}.jsonl").read_bytes().splitlines()
                ) + b"]"
            )
            print(f"{name}: {len(records)} tokens")
        return 0

    args.output_dir.mkdir(parents=True, exist_ok=True)
    for path, content in expected.items():
        path.write_bytes(content)
        print(path)
    print(f"generated tiktoken=={version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
