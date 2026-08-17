#!/usr/bin/env python3
"""S12 release gate driver.

Runs every release workload 1 warmup + 5 isolated measured times (each run a
fresh `mr-crabs-bench` process), aggregates medians and p50/p95/p99 with
nearest-rank arithmetic, compares ONLY byte-identical oracle payloads against
`verification/baselines/oracle-baseline.json`, applies the S12 hard gates
(line loss, process leaks, cache growth), and fails closed on missing or
comparison-ineligible data. Blocked metrics stay visibly blocked and can
never satisfy a gate.

Usage:
  python3 release_gate.py [--bench <binary>] [--baseline <json>]
      [--corpus <json>] [--out <json>] [--workload <name> ...]
  python3 release_gate.py --self-test

Exit codes: 0 = gate PASS, 1 = gate FAIL, 2 = usage/self-test error.
"""

from __future__ import annotations

import argparse
import json
import hashlib
import math
import os
import subprocess
import platform
import sys
import tempfile
from pathlib import Path

SCHEMA_VERSION = 1
WARMUP_RUNS = 1
MEASURED_RUNS = 5
RUN_TIMEOUT_S = 300

WORKLOADS = [
    "ascii_10mb",
    "unicode_10mb",
    "scrollback_1m",
    "resize_storm",
    "redraw_replay",
    "engines_1",
    "engines_10",
    "engines_50",
    "headless_idle",
    "headless_cache",
    "pty_launch_to_prompt",
    "pty_echo",
    "image_decode_stress",
    "effects",
    "search",
    "gui_frame_time",
    "window_redraw",
    "strict_gui_idle",
    "energy",
]

# Workloads whose payloads are pinned in the corpus and may be compared with
# the oracle baseline (identical payload rule).
ORACLE_ELIGIBLE = ["ascii_10mb", "unicode_10mb", "scrollback_1m"]

# Workloads that are expected to report blocked with a reason, by design.
KNOWN_BLOCKED = {
    "gui_frame_time",
    "window_redraw",
    "strict_gui_idle",
    "energy",
}

# Metrics that get p50/p95/p99 aggregation across runs (latency-like).
LATENCY_KEYS = [
    "wall_ns",
    "launch_to_prompt_ns",
    "frame_build_mean_ns",
    "frame_build_p50_ns",
    "frame_build_p95_ns",
    "frame_build_p99_ns",
]

SCROLLBACK_LINES = 1_000_000
ROOT = Path(__file__).resolve().parent.parent.parent


def command_output(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def benchmark_provenance(bench):
    return {
        "source_commit": command_output("git", "rev-parse", "HEAD"),
        "rustc": command_output("rustc", "-Vv"),
        "cargo": command_output("cargo", "-V"),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "processor": platform.processor(),
        "logical_cpus": os.cpu_count(),
        "bench_sha256": hashlib.sha256(bench.read_bytes()).hexdigest(),
    }


# ---------------------------------------------------------------------------
# Arithmetic (mirrors crates/mr-crabs-bench/src/stats.rs)
# ---------------------------------------------------------------------------

def median(values):
    """Median: middle of the sorted sample; even n averages the two middles
    with integer (floor) division, matching the Rust implementation."""
    if not values:
        return None
    s = sorted(values)
    n = len(s)
    if n % 2 == 1:
        return s[n // 2]
    return (s[n // 2 - 1] + s[n // 2]) // 2


def percentile(values, p):
    """Nearest-rank percentile of the sample: rank = ceil(p/100 * n), 1-based."""
    if not values or not (0.0 <= p <= 100.0):
        return None
    rank = max(1, math.ceil(p / 100.0 * len(values)))
    s = sorted(values)
    return s[min(rank, len(s)) - 1]


# ---------------------------------------------------------------------------
# Payload mirrors (byte-identical to crates/mr-crabs-bench/src/payloads.rs)
# ---------------------------------------------------------------------------

FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3


def fnv1a64(data):
    h = FNV_OFFSET
    for b in data:
        h ^= b
        h = (h * FNV_PRIME) & 0xFFFFFFFFFFFFFFFF
    return "%016x" % h


def ascii_byte(j):
    r = j % 32
    if r < 5:
        return [0x1B, ord("["), ord("3"), ord("1"), ord("m")][r]
    if r < 9:
        return [0x1B, ord("["), ord("0"), ord("m")][r - 5]
    return ord("A") + ((j // 32) % 26)


def unicode_byte(j):
    if j + 1 == 10 * 1024 * 1024:
        return ord("A")
    k, r = divmod(j, 48)
    if r < 5:
        return [0x1B, ord("["), ord("3"), ord("1"), ord("m")][r]
    if r < 9:
        return [0x1B, ord("["), ord("0"), ord("m")][r - 5]
    if r < 18:
        c = (r - 9) // 3
        cp = 0x4E00 + ((k * 3 + c) % 0x3000)
        b = (0xE0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3F), 0x80 | (cp & 0x3F))
        return b[(r - 9) % 3]
    return ord("A") + ((k * 7 + r) % 26)


def gen_payload(n, byte_fn):
    return bytes(byte_fn(j) for j in range(n))


def canonical_payloads():
    n10 = 10 * 1024 * 1024
    return {
        "ascii_10mb": gen_payload(n10, ascii_byte),
        "unicode_10mb": gen_payload(n10, unicode_byte),
        "scrollback_1m": b"line\n" * SCROLLBACK_LINES,
    }


# ---------------------------------------------------------------------------
# Result validation (fail closed)
# ---------------------------------------------------------------------------

def validate_run_result(result, workload):
    """Raise ValueError when a run result violates the S12 schema contract."""
    if not isinstance(result, dict):
        raise ValueError("result is not a JSON object")
    if result.get("schema_version") != SCHEMA_VERSION:
        raise ValueError(f"schema_version {result.get('schema_version')!r} != {SCHEMA_VERSION}")
    if result.get("suite") != "release":
        raise ValueError(f"suite {result.get('suite')!r} != 'release'")
    if result.get("workload") != workload:
        raise ValueError(
            f"result workload {result.get('workload')!r} != requested {workload!r}"
        )
    status = result.get("status")
    if status not in ("measured", "failed", "blocked"):
        raise ValueError(f"invalid status {status!r}")
    if status == "measured":
        if not isinstance(result.get("metrics"), dict):
            raise ValueError("measured result missing metrics")
    else:
        if not result.get("reason"):
            raise ValueError(f"{status} result missing exact reason")
    pin = result.get("payload_fnv1a64")
    if pin is not None and not (isinstance(pin, str) and len(pin) == 16):
        raise ValueError(f"invalid payload_fnv1a64 {pin!r}")


def run_one(bench, workload, timeout_s=RUN_TIMEOUT_S):
    """One fresh-process invocation; returns the parsed result dict."""
    with tempfile.TemporaryDirectory(prefix="s12-gate-") as tmp:
        out_path = Path(tmp) / "run.json"
        cmd = [str(bench), "--suite", "release", "--workload", workload, "--json", str(out_path)]
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=timeout_s, check=False
        )
        if proc.returncode != 0:
            raise ValueError(
                f"bench exited {proc.returncode} for {workload}: {proc.stderr.strip()[:500]}"
            )
        if not out_path.exists():
            raise ValueError(f"bench wrote no result file for {workload}")
        with open(out_path, "r", encoding="utf-8") as fh:
            return json.load(fh)


def aggregate_metrics(runs):
    """Median (and p50/p95/p99 for latency keys) across measured runs."""
    keys = set()
    for run in runs:
        keys.update(run["metrics"].keys())
    out = {}
    for key in sorted(keys):
        values = [run["metrics"][key] for run in runs if key in run["metrics"]]
        values = [v for v in values if v is not None]
        if not values:
            continue
        entry = {"median": median(values)}
        if key in LATENCY_KEYS:
            entry.update(
                {
                    "p50": percentile(values, 50.0),
                    "p95": percentile(values, 95.0),
                    "p99": percentile(values, 99.0),
                }
            )
        out[key] = entry
    return out


def median_of(aggregate, key):
    if key not in aggregate:
        return None
    return aggregate[key].get("median")


def all_of(runs, key):
    return all(run["metrics"].get(key) is True for run in runs)


def none_of(runs, key):
    return all(run["metrics"].get(key) is False for run in runs)


# ---------------------------------------------------------------------------
# Gates
# ---------------------------------------------------------------------------

def oracle_gates(workload, aggregate, baseline, corpus):
    """Compare medians against the oracle baseline for identical payloads.

    Returns (compared, verdict_ok, checks, ineligible_reason). Fails closed:
    any missing baseline data or payload-identity mismatch makes the
    comparison ineligible, which is a FAIL.
    """
    pin = corpus["payloads"].get(workload)
    if pin is None:
        return (False, False, {}, f"corpus has no payload pin for {workload}")
    base = baseline.get("workloads", {}).get(workload)
    if not isinstance(base, dict) or "runs" not in base:
        return (False, False, {}, f"oracle baseline has no measured entry for {workload}")

    checks = {}
    ok = True
    for metric, base_key, direction in (
        ("throughput_mib_s", "throughput_mib_s", "ge"),
        ("max_rss_bytes", "median_max_rss_bytes", "le"),
        ("peak_footprint_bytes", "median_peak_footprint_bytes", "le"),
    ):
        ours = median_of(aggregate, metric)
        theirs = base.get(base_key)
        if ours is None or theirs is None:
            ok = False
            checks[metric] = {
                "ok": False,
                "reason": f"missing data (ours={ours}, baseline={theirs})",
            }
            continue
        passed = ours >= theirs if direction == "ge" else ours <= theirs
        check = {"ok": passed, "ours": ours, "baseline": theirs}
        if not passed:
            ok = False
            relation = ">=" if direction == "ge" else "<="
            check["reason"] = f"{ours} is not {relation} oracle {theirs}"
        checks[metric] = check
    return (True, ok, checks, None)


def hard_gate_checks(workload, runs, aggregate, corpus_specs):
    """Workload-specific hard gates; returns (ok, reasons)."""
    reasons = []
    ok = True
    if workload == "scrollback_1m":
        lines = median_of(aggregate, "logical_lines")
        if lines != SCROLLBACK_LINES:
            ok = False
            reasons.append(f"logical_lines {lines} != {SCROLLBACK_LINES} (line loss)")
    elif workload in ("pty_launch_to_prompt", "pty_echo"):
        if not all_of(runs, "child_reaped"):
            ok = False
            reasons.append("child_reaped is not true in every run (process leak)")
        if not none_of(runs, "child_alive_after_reap"):
            ok = False
            reasons.append("child process still alive after reap (process leak)")
    elif workload == "headless_idle":
        if median_of(aggregate, "idle_redraw_requests") != 0:
            ok = False
            reasons.append("idle_redraw_requests != 0 (strict GUI idle violated)")
        if median_of(aggregate, "idle_animation_requests") != 0:
            ok = False
            reasons.append("idle_animation_requests != 0 (strict GUI idle violated)")
        if median_of(aggregate, "capacity_growth_bytes") != 0:
            ok = False
            reasons.append("retained cache capacity grew while idle")
    elif workload == "headless_cache":
        if not all_of(runs, "cache_ok"):
            ok = False
            reasons.append("cache_ok is not true in every run")
        if median_of(aggregate, "capacity_growth_bytes") != 0:
            ok = False
            reasons.append("retained cache capacity grew")
    elif workload == "effects":
        if median_of(aggregate, "frames_after_expiry") != 0:
            ok = False
            reasons.append("frames_after_expiry != 0 (effects still scheduling after expiry)")
        if median_of(aggregate, "effects_disabled_retained_capacity") != 0:
            ok = False
            reasons.append("disabled effects retain heap bytes (must allocate nothing)")
    elif workload == "search":
        spec = corpus_specs.get("search", {})
        expected_matches = spec.get("expected_matches")
        expected_lines = spec.get("lines")
        if expected_matches is not None and median_of(aggregate, "search_matches") != expected_matches:
            ok = False
            reasons.append(
                f"search_matches {median_of(aggregate, 'search_matches')} != expected {expected_matches}"
            )
        if expected_lines is not None and median_of(aggregate, "search_lines_scanned") != expected_lines:
            ok = False
            reasons.append(
                f"search_lines_scanned {median_of(aggregate, 'search_lines_scanned')} != {expected_lines}"
            )
    return ok, reasons


def evaluate_workload(bench, workload, baseline, corpus):
    """Warmup + 5 measured runs, aggregation, and gate evaluation."""
    entry = {"verdict": "FAIL", "status": "unknown", "runs": []}

    # Warmup (discarded; only existence/parse errors fail here).
    try:
        warm = run_one(bench, workload)
        validate_run_result(warm, workload)
    except Exception as exc:  # noqa: BLE001 - fail closed with the reason
        entry.update({"verdict": "FAIL", "status": "failed", "reason": f"warmup: {exc}"})
        return entry

    runs = []
    for _ in range(MEASURED_RUNS):
        try:
            result = run_one(bench, workload)
            validate_run_result(result, workload)
        except Exception as exc:  # noqa: BLE001
            entry.update(
                {"verdict": "FAIL", "status": "failed", "reason": f"measured run: {exc}"}
            )
            return entry
        runs.append(result)
        entry["runs"].append(
            {
                "status": result["status"],
                "payload_fnv1a64": result.get("payload_fnv1a64"),
                "reason": result.get("reason"),
            }
        )

    statuses = {run["status"] for run in runs}
    if workload in KNOWN_BLOCKED:
        if statuses != {"blocked"}:
            entry.update(
                {
                    "verdict": "FAIL",
                    "status": "failed",
                    "reason": f"expected blocked, got statuses {sorted(statuses)}",
                }
            )
            return entry
        entry.update(
            {
                "verdict": "BLOCKED",
                "status": "blocked",
                "reason": runs[0].get("reason") or "not_measured (exact reason in run result)",
            }
        )
        return entry

    if "failed" in statuses:
        reason = next(run["reason"] for run in runs if run["status"] == "failed")
        entry.update({"verdict": "FAIL", "status": "failed", "reason": reason})
        return entry
    if statuses != {"measured"}:
        entry.update(
            {
                "verdict": "FAIL",
                "status": "failed",
                "reason": f"unexpected statuses {sorted(statuses)}",
            }
        )
        return entry

    aggregate = aggregate_metrics(runs)
    entry["aggregate"] = aggregate

    # Payload identity: every run must match the pinned corpus spec.
    pin = corpus.get("payloads", {}).get(workload)
    if pin is not None:
        for run in runs:
            reported = run.get("payload_fnv1a64")
            if reported != pin["fnv1a64"]:
                entry.update(
                    {
                        "verdict": "FAIL",
                        "status": "failed",
                        "reason": (
                            f"payload identity mismatch: run reported {reported}, "
                            f"corpus pins {pin['fnv1a64']}"
                        ),
                    }
                )
                return entry
            if run.get("payload", {}).get("bytes") != pin["bytes"]:
                entry.update(
                    {
                        "verdict": "FAIL",
                        "status": "failed",
                        "reason": (
                            f"payload byte count {run.get('payload', {}).get('bytes')} "
                            f"!= pinned {pin['bytes']}"
                        ),
                    }
                )
                return entry

    # Spec identity: when the corpus pins a replay spec for this workload,
    # every overlapping key must match (the bench may add extra keys, e.g.
    # the actual shell path, but may not contradict the pinned spec).
    corpus_specs = corpus.get("replay", {})
    pinned_spec = corpus_specs.get(workload)
    if pinned_spec is not None:
        for run in runs:
            run_spec = run.get("spec")
            if not isinstance(run_spec, dict):
                entry.update(
                    {
                        "verdict": "FAIL",
                        "status": "failed",
                        "reason": f"workload spec missing; corpus pins {pinned_spec}",
                    }
                )
                return entry
            for key, expected in pinned_spec.items():
                if key in run_spec and run_spec[key] != expected:
                    entry.update(
                        {
                            "verdict": "FAIL",
                            "status": "failed",
                            "reason": (
                                f"spec mismatch on {key!r}: run {run_spec[key]!r} "
                                f"!= corpus {expected!r}"
                            ),
                        }
                    )
                    return entry

    # Hard gates (line loss / leaks / cache growth / strict idle / effects / search).
    ok, reasons = hard_gate_checks(workload, runs, aggregate, corpus_specs)
    if not ok:
        entry.update({"verdict": "FAIL", "status": "failed", "reason": "; ".join(reasons)})
        return entry

    # Oracle comparison (identical payloads only; ineligible == fail closed).
    if workload in ORACLE_ELIGIBLE:
        compared, ok, checks, ineligible = oracle_gates(workload, aggregate, baseline, corpus)
        entry["oracle"] = {
            "compared": compared,
            "checks": checks,
            "ineligible_reason": ineligible,
        }
        if not compared or not ok:
            reason = ineligible or "; ".join(
                f"{name}: {check['reason']}"
                for name, check in checks.items()
                if not check["ok"] and "reason" in check
            )
            entry.update(
                {
                    "verdict": "FAIL",
                    "status": "failed",
                    "reason": f"oracle comparison failed closed: {reason}",
                }
            )
            return entry

    entry["verdict"] = "PASS"
    return entry


# ---------------------------------------------------------------------------
# Self-test
# ---------------------------------------------------------------------------

def self_test(root, corpus_path):
    failures = []

    def check(name, condition, detail=""):
        if not condition:
            failures.append(f"{name}: {detail}")

    fixture_path = corpus_path.parent / "aggregate-math-fixture.json"
    if not fixture_path.exists():
        print("SELF-TEST FAIL: missing aggregate-math-fixture.json")
        return 2
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    for case in fixture["cases"]:
        values = case["values"]
        if "expected_median" in case:
            check(
                f"median[{case['name']}]",
                median(values) == case["expected_median"],
                f"got {median(values)}, expected {case['expected_median']}",
            )
        if "expected_percentiles" in case:
            for label, expected in case["expected_percentiles"].items():
                p = float(label[1:])
                got = percentile(values, p)
                check(
                    f"percentile[{case['name']}][{label}]",
                    got == expected,
                    f"got {got}, expected {expected}",
                )

    corpus = json.loads(corpus_path.read_text(encoding="utf-8"))
    payloads = canonical_payloads()
    for name, payload in payloads.items():
        pin = corpus["payloads"][name]
        check(
            f"payload[{name}].bytes",
            len(payload) == pin["bytes"],
            f"got {len(payload)}, expected {pin['bytes']}",
        )
        got = fnv1a64(payload)
        check(
            f"payload[{name}].fnv1a64",
            got == pin["fnv1a64"],
            f"got {got}, expected {pin['fnv1a64']}",
        )

    for path in sorted((root / "verification").rglob("*.json")):
        try:
            json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            check(f"json[{path.relative_to(root)}]", False, str(error))

    baseline = json.loads(
        (root / "verification" / "baselines" / "oracle-baseline.json").read_text(
            encoding="utf-8"
        )
    )
    baseline_ids = set(baseline.get("workloads", {}))
    check(
        "baseline oracle workload ids",
        set(ORACLE_ELIGIBLE) <= baseline_ids,
        f"missing {sorted(set(ORACLE_ELIGIBLE) - baseline_ids)}",
    )
    check(
        "corpus workload ids",
        set(corpus.get("payloads", {})) == set(ORACLE_ELIGIBLE),
        f"got {sorted(corpus.get('payloads', {}))}",
    )

    for contract_name in ("s11-contract.json", "s12-contract.json"):
        contract = json.loads(
            (root / "verification" / "manifests" / contract_name).read_text(
                encoding="utf-8"
            )
        )
        stale = [path for path in contract.get("owned_paths", []) if path.startswith("rust/")]
        check(f"{contract_name} root paths", not stale, f"stale paths: {stale}")

    if failures:
        print("SELF-TEST FAIL")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print("SELF-TEST PASS (JSON artifacts, contracts, aggregation, and payload pins)")
    return 0


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main(argv):
    parser = argparse.ArgumentParser(description="S12 release gate driver")
    parser.add_argument("--bench", help="path to the mr-crabs-bench binary")
    parser.add_argument("--baseline", help="oracle baseline JSON")
    parser.add_argument("--corpus", help="replay corpus payloads JSON")
    parser.add_argument("--out", help="gate report output JSON")
    parser.add_argument(
        "--workload",
        action="append",
        help="restrict to a workload id (repeatable); default: all",
    )
    parser.add_argument("--self-test", action="store_true", help="run arithmetic/payload self-test")
    args = parser.parse_args(argv)

    root = ROOT
    bench = Path(args.bench) if args.bench else root / "target" / "release" / "mr-crabs-bench"
    baseline_path = Path(args.baseline) if args.baseline else root / "verification" / "baselines" / "oracle-baseline.json"
    corpus_path = Path(args.corpus) if args.corpus else root / "verification" / "corpus" / "replay" / "payloads.json"
    out_path = Path(args.out) if args.out else root / "verification" / "results" / "s12-release-gate.json"

    if args.self_test:
        return self_test(root, corpus_path)

    if not bench.exists():
        print(f"FAIL: bench binary not found: {bench}", file=sys.stderr)
        return 2
    if not corpus_path.exists():
        print(f"FAIL: corpus not found: {corpus_path}", file=sys.stderr)
        return 2
    baseline = {}
    if baseline_path.exists():
        baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
    else:
        print(f"WARNING: baseline not found ({baseline_path}); oracle comparisons will fail closed")
    corpus = json.loads(corpus_path.read_text(encoding="utf-8"))

    workloads = args.workload or WORKLOADS
    for name in workloads:
        if name not in WORKLOADS:
            print(f"FAIL: unknown workload {name}", file=sys.stderr)
            return 2

    report = {
        "schema_version": SCHEMA_VERSION,
        "suite": "release",
        "bench_binary": str(bench),
        "baseline": str(baseline_path),
        "corpus": str(corpus_path),
        "warmup_runs": WARMUP_RUNS,
        "repetitions": MEASURED_RUNS,
        "provenance": benchmark_provenance(bench),
        "workloads": {},
    }

    counts = {"pass": 0, "fail": 0, "blocked": 0}
    for name in workloads:
        entry = evaluate_workload(bench, name, baseline, corpus)
        report["workloads"][name] = entry
        counts[{"PASS": "pass", "FAIL": "fail", "BLOCKED": "blocked"}[entry["verdict"]]] += 1

    # Blocked metrics must be visibly blocked and must not satisfy gates:
    # overall PASS requires every non-blocked workload to pass and the
    # known-blocked set to be exactly the blocked verdicts (when all
    # workloads were requested).
    unexpected_blocked = [
        name for name, entry in report["workloads"].items()
        if entry["verdict"] == "BLOCKED" and name not in KNOWN_BLOCKED
    ]
    expected_blocked_missing = [
        name for name in KNOWN_BLOCKED
        if name in workloads and report["workloads"][name]["verdict"] != "BLOCKED"
    ]
    overall = "PASS" if counts["fail"] == 0 and not unexpected_blocked and not expected_blocked_missing else "FAIL"
    report["overall"] = overall
    report["summary"] = counts
    if unexpected_blocked:
        report["reason"] = f"unexpected blocked workloads: {unexpected_blocked}"
    elif expected_blocked_missing:
        report["reason"] = f"expected blocked workloads not blocked: {expected_blocked_missing}"
    elif counts["fail"]:
        report["reason"] = "one or more workloads failed"

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"overall={overall} pass={counts['pass']} fail={counts['fail']} blocked={counts['blocked']}")
    print(f"report: {out_path}")
    return 0 if overall == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
