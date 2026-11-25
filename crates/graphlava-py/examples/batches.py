"""Large results: batches, IDs only, and Arrow. Run: python examples/batches.py"""

from __future__ import annotations

import pathlib
import tempfile

import graphlava as gl

EX = gl.Namespace("http://ex.org/")
Q = "PREFIX : <http://ex.org/> SELECT ?s ?age WHERE { ?s :age ?age }"


def main(path: pathlib.Path) -> None:
    with gl.Database.open(path) as db:
        db.submit([gl.Set(EX[f"n{i}"], EX.age, i) for i in range(5000)], durability="relaxed")
        rows = db.sparql(Q)
        assert isinstance(rows, gl.Rows)
        total = 0
        for batch in rows.iter_batches():
            total += sum(age for _, age in batch)
        print("sum of ages", total)
        ids = db.datalog("@prefix : <http://ex.org/> .\n?- triple(S, :age, A).").ids()
        print(len(ids), "rows of IDs, nothing decoded")
        arrow_rows = db.sparql(Q)
        assert isinstance(arrow_rows, gl.Rows)
        try:
            table = arrow_rows.to_arrow()
            print(table.num_rows, "rows in Arrow")
        except ImportError:
            print("pip install 'graphlava[arrow]' for to_arrow()")


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as d:
        main(pathlib.Path(d) / "batches.glv")
