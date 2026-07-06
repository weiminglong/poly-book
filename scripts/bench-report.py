#!/usr/bin/env python3
"""Generate docs/PERFORMANCE.md from a completed Criterion run.

Reads target/criterion/**/new/estimates.json (produced by `cargo bench
--workspace`), stamps the result with the machine, toolchain, and commit it
was measured on, and writes a headline table plus the full result set.

Usage: python3 scripts/bench-report.py   (or: just bench-report)
"""

import json
import platform
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRITERION = ROOT / "target" / "criterion"
OUT = ROOT / "docs" / "PERFORMANCE.md"

# Headline rows: criterion id -> (label, items per iteration, pipeline stage).
# The divisor converts a per-iteration median into a per-item cost for benches
# that process a batch each iteration.
HEADLINE = {
    "wire_deser/price_change_delta": ("WS `price_change` deserialize (zero-copy)", 1, "ingest"),
    "wire_deser/book_snapshot_10_levels": ("WS book snapshot deserialize (10 levels)", 1, "ingest"),
    "dispatcher/price_change normalize+shadow-book (200x5 entries)": (
        "Dispatcher normalize + shadow-book cross-check",
        1000,
        "ingest",
    ),
    "codec__encode (book delta)": ("WAL codec encode (book delta)", 1, "durability"),
    "wal_append/append+flush (1k records)": ("WAL append + flush", 1000, "durability"),
    "wal_append/append+fdatasync-each (100 records)": (
        "WAL append + fdatasync every record",
        100,
        "durability",
    ),
    "L2Book__apply_delta": ("Book delta apply", 1, "book"),
    "L2Book__apply_snapshot (50 levels)": ("Book snapshot rebuild (50 levels)", 1, "book"),
    "L2Book__best_bid + best_ask": ("Top-of-book read (best bid + ask)", 1, "book"),
    "LiveReadModel__snapshot (50 levels, depth=20)": (
        "Read-model snapshot (50 levels, depth 20)",
        1,
        "serving",
    ),
    "mixed_workload/10k_deltas_on_20_level_book": (
        "Mixed workload: deltas on a 20-level book",
        10_000,
        "book",
    ),
}


def sh(*argv: str) -> str:
    return subprocess.run(argv, capture_output=True, text=True, check=False).stdout.strip()


def machine() -> dict:
    info = {
        "os": f"{platform.system()} {platform.release()} ({platform.machine()})",
        "rustc": sh("rustc", "--version"),
        "commit": sh("git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"),
        "date": datetime.now(timezone.utc).strftime("%Y-%m-%d"),
    }
    if platform.system() == "Darwin":
        info["cpu"] = sh("sysctl", "-n", "machdep.cpu.brand_string")
        info["cores"] = sh("sysctl", "-n", "hw.ncpu")
        mem = sh("sysctl", "-n", "hw.memsize")
        info["ram"] = f"{int(mem) // (1024**3)} GB" if mem.isdigit() else "unknown"
    else:
        cpu = ""
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
        info["cpu"] = cpu or platform.processor()
        info["cores"] = sh("nproc")
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal"):
                kb = int(line.split()[1])
                info["ram"] = f"{kb // (1024**2)} GB"
                break
    return info


def fmt_ns(ns: float) -> str:
    if ns < 1_000:
        return f"{ns:.0f} ns"
    if ns < 1_000_000:
        return f"{ns / 1_000:.2f} µs"
    if ns < 1_000_000_000:
        return f"{ns / 1_000_000:.2f} ms"
    return f"{ns / 1_000_000_000:.2f} s"


def fmt_rate(per_sec: float) -> str:
    if per_sec >= 1_000_000:
        return f"{per_sec / 1_000_000:.1f} M/s"
    if per_sec >= 1_000:
        return f"{per_sec / 1_000:.0f} k/s"
    return f"{per_sec:.0f}/s"


def collect() -> dict:
    results = {}
    for est in sorted(CRITERION.rglob("new/estimates.json")):
        bench_id = str(est.parent.parent.relative_to(CRITERION))
        data = json.loads(est.read_text())
        results[bench_id] = {
            "median_ns": data["median"]["point_estimate"],
            "mean_ns": data["mean"]["point_estimate"],
            "stddev_ns": data["std_dev"]["point_estimate"],
        }
    return results


def main() -> int:
    if not CRITERION.is_dir():
        print("no target/criterion results; run `cargo bench --workspace` first", file=sys.stderr)
        return 1
    results = collect()
    if not results:
        print("target/criterion contains no estimates.json files", file=sys.stderr)
        return 1
    m = machine()

    lines = []
    a = lines.append
    a("# Performance")
    a("")
    a("Measured Criterion results for the hot-path operations, regenerated with")
    a("`just bench-report` (which runs `cargo bench --workspace` and rewrites this")
    a("file). Numbers are medians of Criterion's sampled iterations on the machine")
    a("below — single-machine, wall-clock measurements for order-of-magnitude")
    a("reasoning, not a controlled lab benchmark. CI compiles every benchmark on")
    a("each PR (`bench` job) but does not gate on timings: shared runners are too")
    a("noisy for statistical regression detection, so regression checks are run")
    a("locally against this file.")
    a("")
    a("## Measurement context")
    a("")
    a(f"- CPU: {m.get('cpu', 'unknown')} ({m.get('cores', '?')} cores), RAM: {m.get('ram', 'unknown')}")
    a(f"- OS: {m['os']}")
    a(f"- Toolchain: {m['rustc']}, bench profile (inherits release: thin LTO, overflow-checks on)")
    a(f"- Commit: `{m['commit']}`, measured {m['date']}")
    a("")
    a("## Pipeline hot path")
    a("")
    a("| Stage | Operation | Median | Per item | Rate |")
    a("|---|---|---|---|---|")
    for bench_id, (label, per, stage) in HEADLINE.items():
        r = results.get(bench_id)
        if not r:
            continue
        median = r["median_ns"]
        per_item = median / per
        a(
            f"| {stage} | {label} | {fmt_ns(median)} | {fmt_ns(per_item)} | "
            f"{fmt_rate(1e9 / per_item)} |"
        )
    a("")
    a("Batch benches (dispatcher, WAL, mixed workload) time the whole batch per")
    a("iteration; the per-item column divides by the batch size.")
    a("")
    a("## All results")
    a("")
    a("| Benchmark | Median | Mean | Std dev |")
    a("|---|---|---|---|")
    for bench_id, r in sorted(results.items()):
        a(
            f"| `{bench_id}` | {fmt_ns(r['median_ns'])} | {fmt_ns(r['mean_ns'])} | "
            f"{fmt_ns(r['stddev_ns'])} |"
        )
    a("")
    a("The ClickHouse-backed cross-backend comparison bench requires a running")
    a("ClickHouse and is not part of this run.")
    a("")

    OUT.write_text("\n".join(lines))
    print(f"wrote {OUT.relative_to(ROOT)} ({len(results)} benchmarks)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
