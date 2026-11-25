"""Every snippet of crates/graphlava-py/README.md, run end to end:
python examples/readme.py"""

from __future__ import annotations

import pathlib
import tempfile

import graphlava as gl
from graphlava import Durability, Namespace

EX = Namespace("http://example.org/")


def main(path: pathlib.Path) -> None:
    # Quick start
    with gl.Database.open(path) as db:
        res = db.submit([gl.Insert(EX.alice, EX.worksAt, EX.acme)], durability=Durability.ORDERED)
        works = res.results[0]                      # a statement ID (an int subclass)
        assert isinstance(works, gl.Id)
        db.submit([gl.Annotate(works, EX.confidence, 0.9)])
        db.submit([gl.Update(works, EX.globex)])    # same ID, annotation kept

        rows = db.sparql(
            "PREFIX : <http://example.org/> "
            "SELECT ?org ?c WHERE { << :alice :worksAt ?org ~ ?r >> . ?r :confidence ?c }"
        )
        assert isinstance(rows, gl.Rows)
        got = list(rows)
        assert got == [(EX.globex, 0.9)], got

        # SPARQL update and Datalog
        db.sparql_update("PREFIX : <http://example.org/> INSERT DATA { :a :knows :b . :b :knows :c }")
        reach = db.datalog(
            "@prefix : <http://example.org/> .\n"
            "reach(?x, ?y) :- :knows(?x, ?y) .\n"
            "reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .\n"
            "?- reach(:a, ?who) ."
        )
        assert sorted(str(w) for (w,) in reach) == [str(EX.b), str(EX.c)]

        # Interactive transaction and snapshot reads
        with db.write(Durability.SYNC) as w:
            w.insert(EX.bob, EX.knows, EX.alice)
        with db.read() as r:
            bob = r.lookup(EX.bob)
            assert bob is not None and len(r.out(bob)) == 1

        # Errors
        try:
            db.submit([gl.Nest(EX.g1, EX.g2), gl.Nest(EX.g2, EX.g1)])
            raise AssertionError("expected a cycle")
        except gl.GraphCycleError as e:
            print("cycle:", e)
    print("README examples ran")


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as d:
        main(pathlib.Path(d) / "memory.glv")
