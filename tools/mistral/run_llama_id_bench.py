#!/usr/bin/env python3

import argparse
import hashlib
import json
import struct
import time
import urllib.request
from pathlib import Path


LANES = [
    "A_per_gid_greedy",
    "B_stream_longest_greedy",
    "C_canonical_tekken",
    "D_exact_min_token_dp",
]

MODES = ["cold", "warm"]


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        while chunk := f.read(1024 * 1024):
            h.update(chunk)
    return h.hexdigest()


def token_sha256(tokens) -> str:
    if not tokens:
        return sha256_bytes(b"")
    blob = struct.pack(f"<{len(tokens)}I", *tokens)
    return sha256_bytes(blob)


def post_json(url: str, payload: bytes, timeout: float):
    req = urllib.request.Request(
        url,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )

    t0 = time.perf_counter_ns()

    with urllib.request.urlopen(req, timeout=timeout) as response:
        body = response.read()

    wall_ns = time.perf_counter_ns() - t0

    return json.loads(body), body, wall_ns


def main():
    ap = argparse.ArgumentParser(
        description=(
            "Run reproducible llama.cpp direct-native-ID inference over "
            "Nereids four-lane request artifacts."
        )
    )

    ap.add_argument(
        "--artifact-dir",
        type=Path,
        required=True,
        help="directory produced by bench_segmentation_lanes.py",
    )

    ap.add_argument(
        "--out-dir",
        type=Path,
        required=True,
        help="directory for full inference responses and report",
    )

    ap.add_argument(
        "--url",
        default="http://127.0.0.1:8081/completion",
    )

    ap.add_argument(
        "--repeats",
        type=int,
        default=3,
    )

    ap.add_argument(
        "--timeout",
        type=float,
        default=300.0,
    )

    args = ap.parse_args()

    if args.repeats < 1:
        raise SystemExit("--repeats must be >= 1")

    source_report_path = args.artifact_dir / "report.json"

    if not source_report_path.is_file():
        raise SystemExit(f"missing {source_report_path}")

    source_report = json.loads(source_report_path.read_text())

    args.out_dir.mkdir(parents=True, exist_ok=True)

    report = {
        "schema": "nereids-llama-native-id-4lane-inference-v1",
        "endpoint": args.url,
        "artifact_dir": str(args.artifact_dir),
        "source_report_sha256": sha256_file(source_report_path),
        "source_sha256": source_report.get("source_sha256"),
        "source_bytes": source_report.get("source_bytes"),
        "temperature": source_report.get("temperature"),
        "ctx": source_report.get("ctx"),
        "requested_generation_tokens": source_report.get("n_predict"),
        "repeats": args.repeats,
        "execution_note": (
            "Requests contain explicit native numeric token IDs. "
            "The inference runner performs no tokenization. "
            "Mode names cold/warm describe the request artifact's "
            "cache_prompt setting; actual runtime reuse is reported "
            "from llama.cpp response fields and must be interpreted "
            "from measured behavior rather than assumed."
        ),
        "runs": [],
        "groups": {},
    }

    for lane in LANES:
        if lane not in source_report["lanes"]:
            raise SystemExit(f"lane missing from source report: {lane}")

        report["groups"][lane] = {}

        for mode in MODES:
            request_path = (
                args.artifact_dir /
                f"{lane}.{mode}.request.json"
            )

            if not request_path.is_file():
                raise SystemExit(f"missing {request_path}")

            request_bytes = request_path.read_bytes()
            request_obj = json.loads(request_bytes)

            prompt = request_obj.get("prompt")
            if not isinstance(prompt, list):
                raise SystemExit(f"{request_path}: prompt is not a list")

            prompt_ids = len(prompt)
            request_sha = sha256_bytes(request_bytes)

            group_runs = []

            print(
                f"\n=== {lane} / {mode} "
                f"prompt_ids={prompt_ids} ===",
                flush=True,
            )

            for repeat in range(1, args.repeats + 1):
                response_obj, response_bytes, wall_ns = post_json(
                    args.url,
                    request_bytes,
                    args.timeout,
                )

                response_path = (
                    args.out_dir /
                    f"{lane}.{mode}.run{repeat}.response.json"
                )

                response_path.write_text(
                    json.dumps(response_obj, indent=2) + "\n"
                )

                output_tokens = response_obj.get("tokens", [])
                timings = response_obj.get("timings", {})

                run = {
                    "lane": lane,
                    "mode": mode,
                    "repeat": repeat,

                    "request_file": str(request_path),
                    "request_sha256": request_sha,
                    "request_prompt_ids": prompt_ids,

                    "response_file": str(response_path),
                    "response_sha256": sha256_file(response_path),

                    "client_wall_ns": wall_ns,
                    "client_wall_ms": wall_ns / 1_000_000.0,

                    "tokens_evaluated":
                        response_obj.get("tokens_evaluated"),

                    "tokens_predicted":
                        response_obj.get("tokens_predicted"),

                    "tokens_cached":
                        response_obj.get("tokens_cached"),

                    "output_token_count":
                        len(output_tokens),

                    "output_tokens_sha256_u32le":
                        token_sha256(output_tokens),

                    "content_sha256":
                        sha256_bytes(
                            response_obj
                            .get("content", "")
                            .encode("utf-8")
                        ),

                    "content":
                        response_obj.get("content"),

                    "stop":
                        response_obj.get("stop"),

                    "stop_type":
                        response_obj.get("stop_type"),

                    "truncated":
                        response_obj.get("truncated"),

                    "id_slot":
                        response_obj.get("id_slot"),

                    "timings": {
                        "cache_n":
                            timings.get("cache_n"),

                        "prompt_n":
                            timings.get("prompt_n"),

                        "prompt_ms":
                            timings.get("prompt_ms"),

                        "prompt_per_token_ms":
                            timings.get("prompt_per_token_ms"),

                        "prompt_per_second":
                            timings.get("prompt_per_second"),

                        "predicted_n":
                            timings.get("predicted_n"),

                        "predicted_ms":
                            timings.get("predicted_ms"),

                        "predicted_per_token_ms":
                            timings.get("predicted_per_token_ms"),

                        "predicted_per_second":
                            timings.get("predicted_per_second"),
                    },
                }

                group_runs.append(run)
                report["runs"].append(run)

                print(
                    "run=%d evaluated=%s predicted=%s "
                    "cache_n=%s prompt_ms=%s gen_ms=%s "
                    "wall_ms=%.3f out=%s"
                    % (
                        repeat,
                        run["tokens_evaluated"],
                        run["tokens_predicted"],
                        run["timings"]["cache_n"],
                        run["timings"]["prompt_ms"],
                        run["timings"]["predicted_ms"],
                        run["client_wall_ms"],
                        run["output_tokens_sha256_u32le"][:16],
                    ),
                    flush=True,
                )

            token_hashes = {
                r["output_tokens_sha256_u32le"]
                for r in group_runs
            }

            content_hashes = {
                r["content_sha256"]
                for r in group_runs
            }

            prompt_ms_values = [
                r["timings"]["prompt_ms"]
                for r in group_runs
                if isinstance(r["timings"]["prompt_ms"], (int, float))
            ]

            predicted_ms_values = [
                r["timings"]["predicted_ms"]
                for r in group_runs
                if isinstance(r["timings"]["predicted_ms"], (int, float))
            ]

            wall_ms_values = [
                r["client_wall_ms"]
                for r in group_runs
            ]

            group = {
                "request_sha256": request_sha,
                "prompt_ids": prompt_ids,
                "repeatability": {
                    "identical_output_token_ids":
                        len(token_hashes) == 1,

                    "identical_output_content":
                        len(content_hashes) == 1,

                    "unique_output_token_hashes":
                        sorted(token_hashes),

                    "unique_content_hashes":
                        sorted(content_hashes),
                },

                "prompt_ms": {
                    "min": min(prompt_ms_values)
                        if prompt_ms_values else None,

                    "mean": (
                        sum(prompt_ms_values) /
                        len(prompt_ms_values)
                    ) if prompt_ms_values else None,

                    "max": max(prompt_ms_values)
                        if prompt_ms_values else None,
                },

                "predicted_ms": {
                    "min": min(predicted_ms_values)
                        if predicted_ms_values else None,

                    "mean": (
                        sum(predicted_ms_values) /
                        len(predicted_ms_values)
                    ) if predicted_ms_values else None,

                    "max": max(predicted_ms_values)
                        if predicted_ms_values else None,
                },

                "client_wall_ms": {
                    "min": min(wall_ms_values),
                    "mean": (
                        sum(wall_ms_values) /
                        len(wall_ms_values)
                    ),
                    "max": max(wall_ms_values),
                },

                "runs": [
                    {
                        "repeat": r["repeat"],
                        "tokens_evaluated":
                            r["tokens_evaluated"],
                        "tokens_predicted":
                            r["tokens_predicted"],
                        "tokens_cached":
                            r["tokens_cached"],
                        "cache_n":
                            r["timings"]["cache_n"],
                        "prompt_ms":
                            r["timings"]["prompt_ms"],
                        "predicted_ms":
                            r["timings"]["predicted_ms"],
                        "client_wall_ms":
                            r["client_wall_ms"],
                        "output_tokens_sha256_u32le":
                            r["output_tokens_sha256_u32le"],
                    }
                    for r in group_runs
                ],
            }

            report["groups"][lane][mode] = group

    # Compare cold/warm output identity inside each lane.
    for lane in LANES:
        cold = report["groups"][lane]["cold"]["repeatability"]
        warm = report["groups"][lane]["warm"]["repeatability"]

        cold_hashes = set(cold["unique_output_token_hashes"])
        warm_hashes = set(warm["unique_output_token_hashes"])

        report["groups"][lane]["cold_vs_warm"] = {
            "same_output_token_hash_set":
                cold_hashes == warm_hashes,
        }

    report_path = args.out_dir / "report.json"
    report_path.write_text(
        json.dumps(report, indent=2) + "\n"
    )

    print("\n=== REPORT ===")
    print(report_path)
    print(sha256_file(report_path), report_path)

    print("\n=== SUMMARY ===")

    for lane in LANES:
        for mode in MODES:
            g = report["groups"][lane][mode]

            print(
                f"{lane:28s} {mode:4s} "
                f"ids={g['prompt_ids']:4d} "
                f"prompt_mean_ms={g['prompt_ms']['mean']} "
                f"wall_mean_ms={g['client_wall_ms']['mean']:.3f} "
                f"repeat_tokens="
                f"{g['repeatability']['identical_output_token_ids']}"
            )


if __name__ == "__main__":
    main()
