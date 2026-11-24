"""Requirement "GIL released during core work"."""

from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import sysconfig
import threading
import time

import pytest

import graphlava as gl
from conftest import EX, PFX, slow_enabled
from graphlava import Durability

N_NODES = int(os.environ.get("GRAPHLAVA_PARALLEL_STATEMENTS", "1000000")) // 4  # 4 statements per node


def build(path: pathlib.Path, nodes: int) -> None:
    db = gl.Database.open(path)
    batch = 10_000
    for start in range(0, nodes, batch // 4):
        ops: list[gl.Op] = []
        for i in range(start, min(nodes, start + batch // 4)):
            n = EX[f"n{i}"]
            ops.append(gl.Insert(n, EX.knows, EX[f"n{(i * 7 + 1) % nodes}"]))
            ops.append(gl.Insert(n, EX.knows, EX[f"n{(i * 13 + 5) % nodes}"]))
            ops.append(gl.Set(n, EX.age, i % 90))
            ops.append(gl.Set(n, EX.name, f"node {i}"))
        db.submit(ops, durability=Durability.RELAXED)
    db.close()


def lookups(db: gl.Database, ids: list[int], knows: int) -> int:
    n = 0
    with db.read() as r:
        for i in ids:
            n += len(r.out(i, knows))
    return n


@pytest.mark.slow
@pytest.mark.skipif(not slow_enabled(), reason="set GRAPHLAVA_SLOW=1: builds a 1M-statement file")
@pytest.mark.xfail(
    strict=False,
    reason="the Rust core's read path does not scale yet: 8 Rust threads take 0.77-0.98x the serial "
    "time on the same file (measured 2026-11-24, macOS arm64), so the binding cannot reach 0.5x",
)
@pytest.mark.skipif((os.cpu_count() or 1) < 8, reason="needs 8 or more cores")
# @tg: verifies:: [[openspec:python-binding#GIL released during core work#Readers run in parallel threads]] {level: e2e}
def test_readers_run_in_parallel(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "big.glv"
    build(path, N_NODES)
    db = gl.Database.open(path)
    with db.read() as r:
        knows = r.lookup(EX.knows)
        assert knows is not None
        ids = [r.lookup(EX[f"n{(i * 7919) % N_NODES}"]) for i in range(1000)]
    assert all(i is not None for i in ids)
    work = [ids] * 8
    t0 = time.perf_counter()
    serial = [lookups(db, w, knows) for w in work]  # type: ignore[arg-type]
    t_serial = time.perf_counter() - t0
    results: list[int] = [0] * 8
    stop = threading.Event()

    def writer() -> None:
        while not stop.is_set():
            db.submit([gl.Set(EX.w, EX.tick, time.time_ns())], durability=Durability.SYNC)

    def reader(k: int) -> None:
        results[k] = lookups(db, work[k], knows)  # type: ignore[arg-type]

    wt = threading.Thread(target=writer)
    wt.start()
    threads = [threading.Thread(target=reader, args=(k,)) for k in range(8)]
    t0 = time.perf_counter()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    t_parallel = time.perf_counter() - t0
    stop.set()
    wt.join()
    db.close()
    assert results == serial, "every thread got the correct rows"
    assert t_parallel < 0.5 * t_serial, f"parallel {t_parallel:.3f}s vs serial {t_serial:.3f}s"


# @tg: verifies:: [[openspec:python-binding#GIL released during core work]] {level: e2e}
def test_waiting_on_the_writer_releases_the_gil(db: gl.Database) -> None:
    """Thread B blocks in ``submit`` waiting for the writer that thread A holds.
    If B kept the GIL while waiting, A could not run its Python code and would
    hit the 100 ms writer limit; with the GIL released A finishes in time."""
    entered = threading.Event()
    errors: list[BaseException] = []

    def holder() -> None:
        try:
            with db.write(Durability.SYNC) as w:
                entered.set()
                time.sleep(0.01)  # let B start waiting on the writer
                for i in range(20):
                    w.insert(EX[f"a{i}"], EX.p, i)
        except BaseException as e:  # noqa: BLE001
            errors.append(e)

    a = threading.Thread(target=holder)
    a.start()
    entered.wait()
    db.submit([gl.Insert(EX.b, EX.p, EX.o)], durability=Durability.SYNC)
    a.join()
    assert errors == []
    assert len(list(db.sparql(PFX + "SELECT ?s WHERE { ?s :p ?o }"))) == 21


FREE_THREADED = bool(sysconfig.get_config_var("Py_GIL_DISABLED"))


@pytest.mark.skipif(not FREE_THREADED, reason="needs a free-threaded (3.14t) interpreter")
# @tg: verifies:: [[openspec:python-binding#GIL released during core work#Free-threaded interpreter]] {level: e2e}
def test_free_threaded_interpreter(tmp_path: pathlib.Path) -> None:
    code = (
        "import sys, pathlib, graphlava\n"
        "sys.path.insert(0, %r)\n"
        "from test_api_parity import agent_memory_session\n"
        "agent_memory_session(pathlib.Path(%r))\n"
        "assert not sys._is_gil_enabled(), 'importing graphlava re-enabled the GIL'\n"
        "print('ok')\n"
    ) % (str(pathlib.Path(__file__).parent), str(tmp_path / "ft.glv"))
    env = dict(os.environ, PYTHON_GIL="0")
    out = subprocess.run([sys.executable, "-c", code], env=env, capture_output=True, text=True)
    assert out.returncode == 0, out.stderr
    assert out.stdout.strip() == "ok"
