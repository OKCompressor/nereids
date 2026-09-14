#!/usr/bin/env python3

import argparse
import hashlib
import json
import struct
import time
from pathlib import Path

from mistral_common.tokens.tokenizers.mistral import MistralTokenizer
from mistral_common.protocol.instruct.messages import UserMessage
from mistral_common.protocol.instruct.request import ChatCompletionRequest


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        while chunk := f.read(1024 * 1024):
            h.update(chunk)
    return h.hexdigest()


def pack_u32(ids):
    if not ids:
        return b""
    return struct.pack(f"<{len(ids)}I", *ids)


def load_model_bin(path: Path):
    blob = path.read_bytes()
    count = struct.unpack_from("<I", blob, 0)[0]
    off = 4
    pieces = []

    for _ in range(count):
        n = struct.unpack_from("<I", blob, off)[0]
        off += 4
        pieces.append(blob[off:off + n])
        off += n

    if off != len(blob):
        raise RuntimeError(
            f"model.bin parse mismatch: parsed={off}, bytes={len(blob)}"
        )

    return pieces


def build_piece_index(pieces):
    piece_to_id = {}
    max_piece = 0

    for tid, piece in enumerate(pieces):
        # Tekken-v3 model/control namespace.
        if tid < 1000 or not piece:
            continue

        # Deterministic handling if byte-identical pieces ever exist.
        piece_to_id.setdefault(piece, tid)
        max_piece = max(max_piece, len(piece))

    return piece_to_id, max_piece


def decode_ids(ids, pieces):
    return b"".join(pieces[i] for i in ids)


def stream_longest_greedy(data, piece_to_id, max_piece):
    """
    Cross-boundary longest-piece heuristic.

    Objective:
        at each byte offset consume the longest legal native piece.

    This is exact-byte but NOT necessarily globally minimum-token
    and NOT canonical BPE.
    """
    result = []
    i = 0

    while i < len(data):
        lim = min(max_piece, len(data) - i)
        match = None

        for n in range(lim, 0, -1):
            tid = piece_to_id.get(data[i:i + n])
            if tid is not None:
                match = (tid, n)
                break

        if match is None:
            raise RuntimeError(f"unmappable byte offset {i}")

        tid, n = match
        result.append(tid)
        i += n

    return result


def exact_min_tokens(data, piece_to_id, max_piece):
    """
    Exact shortest-path / DP oracle.

    Objective:
        minimum number of legal native target-vocabulary IDs.

    This is NOT canonical BPE.
    """
    inf = 10**18
    n_data = len(data)

    dp = [inf] * (n_data + 1)
    prev_pos = [-1] * (n_data + 1)
    prev_id = [-1] * (n_data + 1)

    dp[0] = 0

    for i in range(n_data):
        if dp[i] == inf:
            continue

        lim = min(max_piece, n_data - i)

        # Descending length gives deterministic tie ordering.
        for n in range(lim, 0, -1):
            tid = piece_to_id.get(data[i:i + n])
            if tid is None:
                continue

            j = i + n
            candidate = dp[i] + 1

            if candidate < dp[j]:
                dp[j] = candidate
                prev_pos[j] = i
                prev_id[j] = tid

    if dp[n_data] == inf:
        raise RuntimeError("no exact target-vocabulary path")

    rev = []
    p = n_data

    while p:
        q = prev_pos[p]
        tid = prev_id[p]

        if q < 0 or tid < 1000:
            raise RuntimeError(f"broken DP backtrace at byte offset {p}")

        rev.append(tid)
        p = q

    return list(reversed(rev))


def main():
    ap = argparse.ArgumentParser(
        description=(
            "Build four exact-byte Mistral/Nereids segmentation lanes "
            "over one identical real DU-derived byte prefix."
        )
    )

    ap.add_argument("--du-id-output", type=Path, required=True)
    ap.add_argument("--model-bin", type=Path, required=True)
    ap.add_argument("--raw", type=Path, required=True)
    ap.add_argument("--out-dir", type=Path, required=True)

    ap.add_argument(
        "--lane-a-ids",
        type=int,
        default=2048,
        help="number of Rust per-GID greedy IDs defining the source window",
    )

    ap.add_argument("--ctx", type=int, default=4096)
    ap.add_argument("--n-predict", type=int, default=128)
    ap.add_argument("--temperature", type=float, default=0.0)

    args = ap.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    # ---------------------------------------------------------
    # Target vocabulary
    # ---------------------------------------------------------

    pieces = load_model_bin(args.model_bin)
    vocab_count = len(pieces)
    piece_to_id, max_piece = build_piece_index(pieces)

    # ---------------------------------------------------------
    # Lane A:
    # real DU3 -> current Rust per-GID greedy remapper
    # ---------------------------------------------------------

    requested = args.lane_a_ids

    with args.du_id_output.open("rb") as f:
        packed = f.read(requested * 4)

    if len(packed) != requested * 4:
        raise RuntimeError(
            f"wanted {requested} Rust IDs, got {len(packed) // 4}"
        )

    a_all = list(struct.unpack(f"<{requested}I", packed))

    if not all(1000 <= i < vocab_count for i in a_all):
        raise RuntimeError("lane A contains special/out-of-range IDs")

    # Back off only if the selected boundary lands inside UTF-8.
    used = len(a_all)

    while True:
        lane_a = a_all[:used]
        source = decode_ids(lane_a, pieces)

        try:
            text = source.decode("utf-8")
            if text.encode("utf-8") != source:
                raise RuntimeError("UTF-8 round-trip mismatch")
            break
        except UnicodeDecodeError:
            used -= 1
            if used <= 0:
                raise RuntimeError("could not find UTF-8 boundary")

    with args.raw.open("rb") as f:
        raw_prefix = f.read(len(source))

    if raw_prefix != source:
        raise RuntimeError(
            "Rust DU-derived IDs do not reconstruct the raw prefix"
        )

    # ---------------------------------------------------------
    # Lane B:
    # continuous cross-DU longest-piece greedy
    # ---------------------------------------------------------

    t0 = time.perf_counter_ns()
    lane_b = stream_longest_greedy(
        source,
        piece_to_id,
        max_piece,
    )
    b_ns = time.perf_counter_ns() - t0

    if decode_ids(lane_b, pieces) != source:
        raise RuntimeError("lane B byte reconstruction failed")

    # ---------------------------------------------------------
    # Lane C:
    # official canonical Tekken-v3 oracle
    # ---------------------------------------------------------

    mistral = MistralTokenizer.v3(is_tekken=True)
    tekken = mistral.instruct_tokenizer.tokenizer

    chat_req = ChatCompletionRequest(
        messages=[UserMessage(content=text)]
    )

    t0 = time.perf_counter_ns()
    canonical_full = mistral.encode_chat_completion(chat_req).tokens
    c_ns = time.perf_counter_ns() - t0

    if canonical_full[:2] != [1, 3]:
        raise RuntimeError(
            f"unexpected canonical prefix: {canonical_full[:2]}"
        )

    if canonical_full[-1] != 4:
        raise RuntimeError(
            f"unexpected canonical suffix: {canonical_full[-1:]}"
        )

    lane_c = canonical_full[2:-1]

    canonical_bytes = b"".join(
        tekken.id_to_byte_piece(i)
        for i in lane_c
    )

    if canonical_bytes != source:
        raise RuntimeError("lane C byte reconstruction failed")

    # ---------------------------------------------------------
    # Lane D:
    # mathematically minimum legal native-token count
    # ---------------------------------------------------------

    t0 = time.perf_counter_ns()
    lane_d = exact_min_tokens(
        source,
        piece_to_id,
        max_piece,
    )
    d_ns = time.perf_counter_ns() - t0

    if decode_ids(lane_d, pieces) != source:
        raise RuntimeError("lane D byte reconstruction failed")

    # ---------------------------------------------------------
    # Persist exact source
    # ---------------------------------------------------------

    source_path = args.out_dir / "source.bin"
    source_path.write_bytes(source)

    lanes = {
        "A_per_gid_greedy": lane_a,
        "B_stream_longest_greedy": lane_b,
        "C_canonical_tekken": lane_c,
        "D_exact_min_token_dp": lane_d,
    }

    report = {
        "schema": "nereids-mistral-segmentation-lanes-v1",
        "source": "real DU3-v1g -> Rust id-output -> exact raw prefix",
        "source_bytes": len(source),
        "source_sha256": sha256_bytes(source),
        "source_file": str(source_path),
        "source_file_sha256": sha256_file(source_path),

        "du_id_output": str(args.du_id_output),
        "du_id_output_sha256": sha256_file(args.du_id_output),

        "model_bin": str(args.model_bin),
        "model_bin_sha256": sha256_file(args.model_bin),

        "target_vocab_entries": vocab_count,
        "max_mergeable_piece_bytes": max_piece,

        "ctx": args.ctx,
        "n_predict": args.n_predict,
        "temperature": args.temperature,

        "rust_lane_a_requested_ids": requested,
        "rust_lane_a_used_ids": len(lane_a),

        "construction_ns": {
            "A_per_gid_greedy": None,
            "B_stream_longest_greedy_python": b_ns,
            "C_mistral_common_python": c_ns,
            "D_exact_min_token_dp_python": d_ns,
        },

        "construction_note": (
            "Lane A already exists as Rust output. "
            "B and D are Python experimental/oracle implementations. "
            "C is the mistral-common canonical oracle. "
            "These timings are not cross-language production benchmarks."
        ),

        "lanes": {},
    }

    # ---------------------------------------------------------
    # Persist .u32 lanes and cold/warm inference requests
    # ---------------------------------------------------------

    for name, ids in lanes.items():
        ids_blob = pack_u32(ids)
        ids_path = args.out_dir / f"{name}.u32"
        ids_path.write_bytes(ids_blob)

        if name == "C_canonical_tekken":
            rebuilt = b"".join(
                tekken.id_to_byte_piece(i)
                for i in ids
            )
        else:
            rebuilt = decode_ids(ids, pieces)

        if rebuilt != source:
            raise RuntimeError(f"{name}: exact byte check failed")

        prompt = [1, 3] + ids + [4]

        total_prompt = len(prompt)
        total_with_generation = total_prompt + args.n_predict
        margin = args.ctx - total_with_generation

        if margin < 0:
            raise RuntimeError(
                f"{name}: prompt={total_prompt} + "
                f"n_predict={args.n_predict} exceeds ctx={args.ctx}"
            )

        lane_info = {
            "content_ids": len(ids),
            "model_control_ids": 3,
            "total_prompt_ids": total_prompt,
            "bytes_per_content_id": len(source) / len(ids),
            "exact_bytes": True,

            "ids_file": str(ids_path),
            "ids_sha256_u32le": sha256_bytes(ids_blob),

            "first_ids": ids[:16],
            "last_ids": ids[-16:],

            "ctx_with_requested_generation": total_with_generation,
            "ctx_margin": margin,
        }

        report["lanes"][name] = lane_info

        common = {
            "prompt": prompt,
            "n_predict": args.n_predict,
            "temperature": args.temperature,
            "return_tokens": True,
            "timings_per_token": True,
        }

        cold = dict(common)
        cold["cache_prompt"] = False

        warm = dict(common)
        warm["cache_prompt"] = True

        cold_path = args.out_dir / f"{name}.cold.request.json"
        warm_path = args.out_dir / f"{name}.warm.request.json"

        cold_path.write_text(
            json.dumps(cold, separators=(",", ":")) + "\n"
        )

        warm_path.write_text(
            json.dumps(warm, separators=(",", ":")) + "\n"
        )

        lane_info["cold_request"] = str(cold_path)
        lane_info["cold_request_sha256"] = sha256_file(cold_path)

        lane_info["warm_request"] = str(warm_path)
        lane_info["warm_request_sha256"] = sha256_file(warm_path)

    # ---------------------------------------------------------
    # Comparison metrics
    # ---------------------------------------------------------

    a = len(lane_a)
    b = len(lane_b)
    c = len(lane_c)
    d = len(lane_d)

    report["comparisons"] = {
        "B_vs_A_token_reduction_pct":
            100.0 * (a - b) / a,

        "C_vs_A_token_reduction_pct":
            100.0 * (a - c) / a,

        "D_vs_A_token_reduction_pct":
            100.0 * (a - d) / a,

        "B_minus_C_tokens":
            b - c,

        "B_minus_C_pct_of_C":
            100.0 * (b - c) / c,

        "B_greedy_regret_tokens_vs_exact_min":
            b - d,

        "B_greedy_regret_pct_vs_exact_min":
            100.0 * (b - d) / d,

        "C_canonical_minus_exact_min_tokens":
            c - d,

        "C_canonical_regret_pct_vs_exact_min":
            100.0 * (c - d) / d,
    }

    report_path = args.out_dir / "report.json"
    report_path.write_text(
        json.dumps(report, indent=2) + "\n"
    )

    print(json.dumps(report, indent=2))

    print("\nARTIFACT SHA256")
    for p in sorted(args.out_dir.iterdir()):
        if p.is_file():
            print(sha256_file(p), p)


if __name__ == "__main__":
    main()
