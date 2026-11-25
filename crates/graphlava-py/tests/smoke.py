"""Wheel smoke test: run in a clean virtualenv on every wheel target.

``python smoke.py [path]`` runs the agent memory session against an installed
``graphlava`` and checks the version; exits non-zero on failure.
"""

# @tg: verifies:: [[openspec:python-binding#Python packaging#Wheel install and smoke test per target]] {level: e2e}

from __future__ import annotations

import pathlib
import sys
import tempfile

import graphlava as gl

EX = gl.Namespace("http://ex.org/")
Q = "PREFIX : <http://ex.org/> SELECT ?c WHERE { << :alice :knows :bob >> :confidence ?c }"


def main(path: pathlib.Path) -> None:
    assert gl.__version__ == gl.core_version(), (gl.__version__, gl.core_version())
    db = gl.Database.open(path)
    sid = db.submit([gl.Insert(EX.alice, EX.knows, EX.bob)], durability=gl.Durability.ORDERED).results[0]
    assert isinstance(sid, int) and 0 <= sid < 2**64
    with db.write(gl.Durability.ORDERED) as w:
        w.annotate(sid, EX.confidence, 0.9)
    db.close()
    db = gl.Database.open(path)
    rows = list(db.sparql(Q))
    assert rows == [(0.9,)], rows
    with db.read() as r:
        a, k = r.lookup(EX.alice), r.lookup(EX.knows)
        assert a is not None and k is not None
        assert [m.id for m in r.out(a, k)] == [sid]
    db.close()
    print(f"graphlava {gl.__version__}: smoke test passed")


if __name__ == "__main__":
    if len(sys.argv) > 1:
        main(pathlib.Path(sys.argv[1]))
    else:
        with tempfile.TemporaryDirectory() as d:
            main(pathlib.Path(d) / "smoke.glv")
