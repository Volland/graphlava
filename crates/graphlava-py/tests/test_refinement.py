"""Oracle trace replay through Python (L1, mirrors crates/graphlava/tests/refinement.rs).

``lake exe oracle <seed> <n>`` prints the Lean model's result for each
operation. The Rust replay checks the Rust store against it op by op; this test
replays the same traces through the Python API and checks every result and the
final state the same way, so Python results equal the Rust replay results.
The abstraction function is the one of refinement.rs: model node n ↔
``http://oracle/n{n}``, predicate p ↔ ``http://oracle/p{p}``, lit n ↔ the
integer n, statement and hyperedge IDs ↔ Rust IDs by a bijection built during
the replay. Seeds 1..16 × 2,000 operations (``GRAPHLAVA_PY_REFINEMENT_SEEDS``,
``GRAPHLAVA_PY_REFINEMENT_OPS`` override).
"""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import subprocess
from typing import Any

import pytest

import graphlava as gl

NS = "http://oracle/"
FORMAL = pathlib.Path(__file__).resolve().parents[3] / "formal"
DEFAULT_GRAPH = 0x5000_0000_0000_0001


def oracle(seed: int, n: int) -> list[dict[str, Any]] | None:
    binary = FORMAL / ".lake" / "build" / "bin" / "oracle"
    if binary.exists():
        cmd = [str(binary), str(seed), str(n)]
    elif shutil.which("lake"):
        cmd = ["lake", "exe", "oracle", str(seed), str(n)]
    else:
        return None
    out = subprocess.run(cmd, cwd=FORMAL, capture_output=True, text=True, check=True)
    return [json.loads(line) for line in out.stdout.splitlines()]


def iri(local: str) -> gl.IRI:
    return gl.IRI(NS + local)


class Replay:
    def __init__(self, db: gl.Database) -> None:
        self.db = db
        self.to_rust: dict[int, gl.Id] = {}
        self.to_model: dict[int, int] = {}

    def graph(self, g: int) -> Any:
        return gl.Id(DEFAULT_GRAPH) if g == 0 else iri(f"g{g}")

    def term(self, v: dict[str, Any]) -> Any:
        if "node" in v:
            return iri(f"n{v['node']}")
        if "lit" in v:
            return int(v["lit"])
        return self.to_rust[v["stmt"]]

    def rust_id(self, model: int) -> gl.Id:
        # an ID the model never allocated maps to one Rust never allocates
        return self.to_rust.get(model, gl.Id((4 << 60) | ((1 << 40) - 1) << 20 | (model % (1 << 20))))

    def bind(self, expect: int, rid: gl.Id, ctx: str) -> None:
        if expect in self.to_rust:
            assert self.to_rust[expect] == rid, f"{ctx}: existing item"
        else:
            assert int(rid) not in self.to_model, f"{ctx}: Rust returned an existing ID for a fresh model ID"
            self.to_rust[expect] = rid
            self.to_model[int(rid)] = expect

    def submit(self, op: gl.Op) -> Any:
        return self.db.submit([op], durability=gl.Durability.RELAXED).results[0]

    def removed(self, gone: list[gl.Id]) -> set[int]:
        return {self.to_model[int(g)] for g in gone}

    def step(self, line: dict[str, Any], at: int) -> None:
        ctx = f"op {at}: {line}"
        op = line["op"]
        e = line.get("expect", {})
        if op == "insert":
            t = line["triple"]
            s, o, p = self.term(t["s"]), self.term(t["o"]), iri(f"p{t['p']}")
            rid = self.submit(gl.Insert(s, p, o, graph=self.graph(line["graph"])))
            if rid is None:
                rid = self.submit(gl.StatementId(s, p, o))
            assert isinstance(rid, gl.Id), ctx
            self.bind(e["id"], rid, ctx)
        elif op == "hedge":
            roles = [(iri(f"r{r[0]}"), self.term(r[1])) for r in line["roles"]]
            hop = gl.CreateHedge(iri(f"T{line['type']}"), roles=roles, graph=self.graph(line["graph"]))
            if e.get("id") is None:
                with pytest.raises(gl.HyperedgeShapeMismatchError):
                    self.submit(hop)
            else:
                rid = self.submit(hop)
                assert isinstance(rid, gl.Id) and rid.kind == "hyperedge", ctx
                self.bind(e["id"], rid, ctx)
        elif op in ("add_member", "remove_member"):
            item = self.rust_id(line["id"])
            if item.kind == "hyperedge":
                assert e["ok"] is False, ctx
                return
            cls = gl.AddMember if op == "add_member" else gl.RemoveMember
            try:
                res = self.submit(cls(item, self.graph(line["graph"])))
                ok = True
            except (gl.ReferencedByStatementError, gl.FillsRoleError):
                assert e["ok"] is False and e.get("blocked") is True, ctx
                return
            except (gl.NotFoundError, gl.InvalidError):
                assert e["ok"] is False and "blocked" not in e, ctx
                return
            assert ok == e["ok"], ctx
            if op == "remove_member":
                assert self.removed(res) == set(e["removed"]), f"{ctx}: removed set"
        elif op in ("nest", "unnest"):
            child, parent = self.graph(line["child"]), self.graph(line["parent"])
            try:
                res = self.submit(gl.Nest(child, parent) if op == "nest" else gl.Unnest(child, parent))
                ok = True if op == "nest" else res is True
            except gl.GraphCycleError:
                assert op == "nest", ctx
                ok = False
            except gl.InvalidError:
                assert op == "unnest", ctx
                ok = False
            assert ok == e["ok"], ctx
        elif op == "scope":
            with self.db.read() as r:
                g = gl.Id(DEFAULT_GRAPH) if line["graph"] == 0 else r.graph(iri(f"g{line['graph']}"))
                got = [] if g is None else sorted(
                    self.to_model[int(i)] for i in r.scope_statements(g, line["direct"])
                )
            assert got == e["ids"], f"{ctx}: scope"
        elif op == "update":
            try:
                self.submit(gl.Update(self.rust_id(line["id"]), self.term(line["o"])))
                ok = True
            except (gl.ConflictError, gl.NotFoundError):
                ok = False
            assert ok == e["ok"], ctx
        elif op == "delete":
            mode = gl.DeleteMode.DETACH if line["detach"] else gl.DeleteMode.CASCADE
            try:
                res = self.submit(gl.Delete(self.rust_id(line["id"]), mode))
            except (gl.ReferencedByStatementError, gl.FillsRoleError):
                assert e["ok"] is False and e.get("blocked") is True, ctx
                return
            except gl.NotFoundError:
                assert e["ok"] is False and "blocked" not in e, ctx
                return
            assert e["ok"] is True, ctx
            assert self.removed(res) == set(e["removed"]), f"{ctx}: removed set"
        else:
            raise AssertionError(f"unknown op {op}")

    def check_final(self, line: dict[str, Any], seed: int) -> None:
        with self.db.read() as r:

            def term_json(i: gl.Id) -> dict[str, Any]:
                if i.kind == "statement":
                    return {"stmt": self.to_model[int(i)]}
                v = r.decode(i)
                if isinstance(v, int):
                    return {"lit": v}
                assert isinstance(v, gl.IRI)
                return {"node": int(v.value.removeprefix(NS + "n"))}

            got = []
            for m in r.find(None, None, None):
                p = r.decode(m.p)
                if not (isinstance(p, gl.IRI) and p.value.startswith(NS + "p")):
                    continue
                assert m.id is not None
                got.append(
                    (self.to_model[int(m.id)],
                     {"s": term_json(m.s), "p": int(p.value.removeprefix(NS + "p")), "o": term_json(m.o)})
                )
            want = [(x[0], x[1]) for x in line["final"]]
            assert sorted(got, key=lambda x: x[0]) == sorted(want, key=lambda x: x[0]), f"seed {seed}: final state"

            def graph_no(g: gl.Id) -> int:
                if g == DEFAULT_GRAPH:
                    return 0
                v = r.decode(g)
                assert isinstance(v, gl.IRI)
                return int(v.value.removeprefix(NS + "g"))

            for item, graphs in line["members"]:
                gs = sorted(graph_no(g) for g in r.graphs_of(self.to_rust[item]))
                assert gs == graphs, f"seed {seed}: memberships of {item}"
            live = 0
            for hid, h in line["hedges"]:
                he = r.hedge(self.to_rust[hid])
                assert he is not None, f"seed {seed}: hyperedge {hid} is gone"
                assert r.decode(he.type) == iri(f"T{h['type']}")
                roles = sorted((r.decode(role).value, term_json(n)) for role, n in he.roles)  # type: ignore[union-attr]
                assert roles == sorted((NS + f"r{x[0]}", x[1]) for x in h["roles"]), f"seed {seed}: roles"
                live += 1
            assert live == sum(1 for i in self.to_rust.values() if i.kind == "hyperedge" and r.hedge(i) is not None)
        assert len(self.to_rust) + 1 == line["next"], f"seed {seed}: allocator"


SEEDS = int(os.environ.get("GRAPHLAVA_PY_REFINEMENT_SEEDS", "16"))
OPS = int(os.environ.get("GRAPHLAVA_PY_REFINEMENT_OPS", "2000"))


@pytest.mark.parametrize("seed", range(1, SEEDS + 1))
# @tg: verifies:: [[openspec:python-binding#Python API parity#Oracle trace replay through Python]] {level: refinement}
def test_oracle_replay(seed: int, tmp_path: pathlib.Path) -> None:
    trace = oracle(seed, OPS)
    if trace is None:
        if os.environ.get("GRAPHLAVA_REQUIRE_REFINEMENT"):
            pytest.fail("oracle not built and `lake` not on PATH")
        pytest.skip("oracle not built and `lake` not on PATH")
    path = tmp_path / "replay.glv"
    db = gl.Database.open(path)
    schema = trace[0]["schema"]
    for p in schema["multi"]:
        db.submit([gl.DeclarePredicate(iri(f"p{p}"), multi=True)], durability=gl.Durability.RELAXED)
    for t in schema["htypes"]:
        roles = [gl.Role(iri(f"r{r}"), required=r in t["required"]) for r in t["roles"]]
        db.submit([gl.DeclareHedgeType(iri(f"T{t['id']}"), roles=roles)], durability=gl.Durability.RELAXED)
    rp = Replay(db)
    for i, line in enumerate(trace[1:-1]):
        rp.step(line, i)
        if i == 1000:  # close and reopen mid-trace, as the Rust replay does
            rp.db.close()
            rp.db = gl.Database.open(path)
    rp.check_final(trace[-1], seed)
    rp.db.close()
