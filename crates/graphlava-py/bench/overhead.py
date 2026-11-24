"""Binding overhead benchmark for Python ([[17-benchmarks#Binding overhead]]).

Builds the dataset (seed 42), runs the Rust baseline
(``cargo run --release -p graphlava-py --example overhead_baseline``) and the
same calls through Python on the same file, and writes the per-call overhead
above Rust (p50/p99/p99.9 µs, with log-linear HDR-style histograms) to a JSON
result. ``--baseline FILE`` gates: the run fails when the p50 overhead of
``props`` exceeds the baseline by more than 20% at 95% confidence (bootstrap
over ``--runs`` repetitions).

    python bench/overhead.py --statements 1000000 --runs 5 --out result.json
    python bench/overhead.py --statements 20000 --runs 1      # short local check

Needs ``graphlava`` installed in the running interpreter (``maturin develop --release``).
"""

# @tg: implements:: [[openspec:python-binding#Python call overhead]]
# @tg: verifies:: [[openspec:python-binding#Python call overhead#Overhead benchmark runs nightly]] {level: benchmark}
# @tg: verifies:: [[openspec:python-binding#Python call overhead#Batch path amortises overhead]] {level: benchmark}

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import platform
import random
import statistics
import subprocess
import sys
import tempfile
import time
from typing import Any

import graphlava as gl

EX = gl.Namespace("http://ex.org/")
CRATE = pathlib.Path(__file__).resolve().parents[1]
ROOT = CRATE.parents[1]
Q = "PREFIX : <http://ex.org/> SELECT ?s ?age WHERE { ?s :age ?age } LIMIT 10000"


class Lcg:
    """The node sequence of rust_baseline.rs: a 64-bit LCG from seed 42."""

    def __init__(self, seed: int) -> None:
        self.s = seed

    def next(self, n: int) -> int:
        self.s = (self.s * 6364136223846793005 + 1442695040888963407) % (1 << 64)
        return (self.s >> 33) % n


def build(path: pathlib.Path, statements: int) -> int:
    """`py-agent-v1`: 4 statements per node (two `knows` edges, `age`, `name`), seed 42."""
    nodes = statements // 4
    rnd = random.Random(42)
    with gl.Database.open(path) as db:
        for start in range(0, nodes, 2500):
            ops: list[gl.Op] = []
            for i in range(start, min(nodes, start + 2500)):
                n = EX[f"n{i}"]
                ops += [
                    gl.Insert(n, EX.knows, EX[f"n{rnd.randrange(nodes)}"]),
                    gl.Insert(n, EX.knows, EX[f"n{rnd.randrange(nodes)}"]),
                    gl.Set(n, EX.age, rnd.randrange(90)),
                    gl.Set(n, EX.name, f"node {i}"),
                ]
            db.submit(ops, durability=gl.Durability.RELAXED)
    return nodes


def summary(ns: list[int], per: int = 1) -> dict[str, Any]:
    ns = sorted(ns)

    def q(p: float) -> float:
        return ns[round((len(ns) - 1) * p)] / 1000 / per

    hist: dict[int, int] = {}
    for v in ns:
        v = max(v, 1)
        e = v.bit_length() - 1
        sub = (v >> (e - 4)) & 15 if e >= 4 else v & 15
        hist[e * 16 + sub] = hist.get(e * 16 + sub, 0) + 1
    return {"n": len(ns), "p50_us": q(0.5), "p99_us": q(0.99), "p999_us": q(0.999),
            "hist_log16_ns": sorted(hist.items())}


def python_run(path: pathlib.Path, nodes: int, ops: int) -> dict[str, Any]:
    res: dict[str, Any] = {}
    pc = time.perf_counter_ns
    with gl.Database.open(path) as db:
        with db.read() as r:
            knows = r.lookup(EX.knows)
            rng = Lcg(42)
            ids = [r.lookup(EX[f"n{rng.next(nodes)}"]) for _ in range(ops)]
            s = []
            for i in ids:
                t = pc()
                r.props(i)  # type: ignore[arg-type]
                s.append(pc() - t)
            res["props"] = summary(s)
            s = []
            for i in ids:
                t = pc()
                for m in r.out(i, knows):  # type: ignore[arg-type]
                    r.out(m.o, knows)
                s.append(pc() - t)
            res["out_2hop"] = summary(s)
        row_s, batch_s = [], []
        for _ in range(5):
            t = pc()
            rows = db.sparql(Q)
            assert isinstance(rows, gl.Rows)
            n = sum(1 for _ in rows)
            row_s.append((pc() - t) // max(n, 1))
            t = pc()
            rows = db.sparql(Q)
            assert isinstance(rows, gl.Rows)
            n = sum(len(b) for b in rows.iter_batches())
            batch_s.append((pc() - t) // max(n, 1))
        res["select_10k_per_row"] = summary(row_s)
        res["select_10k_per_row_batched"] = summary(batch_s)
        for name, d in (("submit_1_ordered", gl.Durability.ORDERED), ("commit_sync", gl.Durability.SYNC),
                        ("commit_relaxed", gl.Durability.RELAXED)):
            n = min(ops, 500) if d is gl.Durability.SYNC else min(ops, 2000)
            s = []
            for i in range(n):
                op = gl.Set(EX[f"n{i % nodes}"], EX.tick, i)
                t = pc()
                db.submit([op], durability=d)
                s.append(pc() - t)
            res[name] = summary(s)
    return res


def rust_run(path: pathlib.Path, nodes: int, ops: int) -> dict[str, Any]:
    out = subprocess.run(
        ["cargo", "run", "-q", "--release", "-p", "graphlava-py", "--example", "overhead_baseline", "--",
         str(path), str(nodes), str(ops)],
        cwd=ROOT, check=True, capture_output=True, text=True,
    )
    return json.loads(out.stdout)  # type: ignore[no-any-return]


def bootstrap_lower(ratios: list[float], n: int = 2000) -> float:
    """Lower end of the 95% bootstrap CI of the median ratio."""
    rnd = random.Random(7)
    meds = sorted(statistics.median(rnd.choices(ratios, k=len(ratios))) for _ in range(n))
    return meds[int(0.025 * n)]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--statements", type=int, default=1_000_000)
    ap.add_argument("--ops", type=int, default=100_000)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--out", type=pathlib.Path)
    ap.add_argument("--baseline", type=pathlib.Path)
    a = ap.parse_args()
    runs = []
    with tempfile.TemporaryDirectory() as d:
        path = pathlib.Path(d) / "overhead.glv"
        nodes = build(path, a.statements)
        for _ in range(a.runs):
            rust = rust_run(path, nodes, a.ops)
            py = python_run(path, nodes, a.ops)
            over = {k: {p: py[k][p] - rust[k][p] for p in ("p50_us", "p99_us", "p999_us")}
                    for k in rust if k in py}
            over["select_10k_per_row_batched"] = {
                p: py["select_10k_per_row_batched"][p] - rust["select_10k_per_row"][p]
                for p in ("p50_us", "p99_us", "p999_us")
            }
            runs.append({"rust": rust, "python": py, "overhead_us": over})
    result = {
        "benchmark": "Binding overhead", "binding": "python", "dataset": "py-agent-v1",
        "seed": 42, "statements": a.statements, "ops": a.ops, "runs": runs,
        "env": {"python": sys.version, "platform": platform.platform(), "machine": platform.machine(),
                "graphlava": gl.__version__,
                "commit": subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True,
                                         text=True).stdout.strip(),
                "date": dt.datetime.now(dt.timezone.utc).isoformat()},
    }
    p50s = [r["overhead_us"]["props"]["p50_us"] for r in runs]
    result["props_overhead_p50_us_median"] = statistics.median(p50s)
    text = json.dumps(result, indent=1)
    if a.out:
        a.out.write_text(text)
    for k, v in runs[-1]["overhead_us"].items():
        print(f"{k:28s} overhead p50 {v['p50_us']:9.3f} µs  p99 {v['p99_us']:9.3f} µs")
    if a.baseline:
        base = json.loads(a.baseline.read_text())
        ref = base.get("props_overhead_p50_us_median")
        if ref is None:
            print("baseline not recorded yet: this run can become it")
            return 0
        lower = bootstrap_lower([p / ref for p in p50s])
        print(f"props p50 overhead vs baseline: lower 95% bound {lower:.3f}x")
        if lower > 1.2:
            print("FAIL: p50 overhead regressed by more than 20%")
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
