"""Crash test: a Python child commits with ``sync``, prints the ID and is killed."""

from __future__ import annotations

import os
import pathlib
import signal
import subprocess
import sys
import textwrap

import graphlava as gl
from conftest import EX

CHILD = textwrap.dedent(
    """
    import sys, time
    import graphlava as gl
    EX = gl.Namespace("http://ex.org/")
    db = gl.Database.open(sys.argv[1])
    with db.write(gl.Durability.SYNC) as w:
        sid = w.insert(EX.alice, EX.knows, EX.bob)
    print(int(sid), flush=True)
    # an unacknowledged relaxed write after the acknowledged one
    db.submit([gl.Insert(EX.alice, EX.knows, EX.carol)], durability=gl.Durability.RELAXED)
    time.sleep(60)  # killed here, before close
    """
)


# @tg: verifies:: [[openspec:python-binding#Python transactions as context managers#Python process killed after a sync commit]] {level: crash}
def test_sigkill_after_sync_commit(path: pathlib.Path) -> None:
    for round_ in range(3):
        child = subprocess.Popen(
            [sys.executable, "-c", CHILD, str(path)], stdout=subprocess.PIPE, text=True
        )
        assert child.stdout is not None
        line = child.stdout.readline().strip()
        os.kill(child.pid, signal.SIGKILL)
        child.wait()
        assert line, f"round {round_}: the child printed no ID"
        sid = int(line)
        db = gl.Database.open(path)  # no recovery error
        with db.read() as r:
            got = r.stmt(sid)
            assert got is not None, f"round {round_}: acknowledged statement {sid:#x} is lost"
            assert r.decode(got[0]) == EX.alice and r.decode(got[2]) == EX.bob
        db.close()
