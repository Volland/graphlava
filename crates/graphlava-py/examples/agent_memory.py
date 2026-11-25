"""Agent memory with graphlava: one-shot writes first, an interactive block for
read-your-writes, then SPARQL. Run: python examples/agent_memory.py"""

from __future__ import annotations

import datetime as dt
import pathlib
import tempfile

import graphlava as gl
from graphlava import Durability, Namespace

EX = Namespace("http://ex.org/")


def remember(db: gl.Database) -> gl.Id:
    """Writes a fact and its provenance in one atomic request."""
    res = db.submit(
        [
            gl.Insert(EX.alice, EX.worksAt, EX.acme, graph=EX.episode1),
            gl.Set(EX.alice, EX.age, 42),
            gl.Set(EX.alice, EX.seen, dt.datetime.now(dt.timezone.utc)),
        ],
        durability=Durability.ORDERED,
    )
    sid = res.results[0]
    assert isinstance(sid, gl.Id)
    return sid


def main(path: pathlib.Path) -> None:
    with gl.Database.open(path) as db:
        sid = remember(db)
        with db.write(Durability.SYNC) as w:  # short: the writer is held for at most 100 ms
            w.annotate(sid, EX.confidence, 0.9)
            w.update(sid, EX.globex)  # same statement ID, new object
        rows = db.sparql(
            "PREFIX : <http://ex.org/> "
            "SELECT ?o ?c WHERE { << :alice :worksAt ?o >> :confidence ?c }"
        )
        assert not isinstance(rows, bool)
        for o, c in rows:
            print(o, c)
        with db.read() as r:
            alice = r.lookup(EX.alice)
            if alice is not None:
                for m in r.props(alice):
                    print(r.decode(m.p), "=", r.decode(m.o))
        try:
            db.submit([gl.Update(sid, EX.initech, expected=EX.acme)])
        except gl.ConflictError:
            print("compare-and-set lost: the value changed meanwhile")


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as d:
        main(pathlib.Path(d) / "memory.glv")
