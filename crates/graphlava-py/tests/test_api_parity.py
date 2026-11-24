"""Requirement "Python API parity"."""

from __future__ import annotations

import pathlib

import pytest

import graphlava as gl
from conftest import EX, PFX, reopen
from graphlava import Durability


def agent_memory_session(path: pathlib.Path) -> None:
    """The agent memory session, shared with the wheel smoke test."""
    db = gl.Database.open(path)
    res = db.submit([gl.Insert(EX.alice, EX.knows, EX.bob)], durability=Durability.ORDERED)
    sid = res.results[0]
    assert isinstance(sid, int) and 0 <= sid < 2**64
    with db.write(Durability.ORDERED) as w:
        w.annotate(sid, EX.confidence, 0.9)
    db = reopen(db, path)
    rows = list(db.sparql(PFX + "SELECT ?c WHERE { << :alice :knows :bob >> :confidence ?c }"))
    assert rows == [(0.9,)]
    with db.read() as r:
        m = r.out(r.lookup(EX.alice), r.lookup(EX.knows))
        assert [x.id for x in m] == [sid], "the statement ID survives the reopen"
    db.close()


# @tg: verifies:: [[openspec:python-binding#Python API parity#Agent memory session from Python]] {level: e2e}
# @tg: verifies:: [[21-e2e-tests#Python agent memory session]] {level: e2e}
def test_agent_memory_session(path: pathlib.Path) -> None:
    agent_memory_session(path)


# @tg: verifies:: [[openspec:python-binding#Python API parity#Unknown keyword argument]] {level: unit}
def test_unknown_durability_value(db: gl.Database) -> None:
    with pytest.raises(ValueError) as e:
        db.write(durability="fast")  # type: ignore[arg-type]
    msg = str(e.value)
    for accepted in ("sync", "ordered", "relaxed"):
        assert accepted in msg
    # no write transaction was opened: a one-shot write goes straight through
    db.submit([gl.Insert(EX.a, EX.p, EX.b)], durability="sync")
    with pytest.raises(TypeError):
        db.write(durabilty=Durability.SYNC)  # type: ignore[call-arg]


# @tg: verifies:: [[openspec:python-binding#Python API parity]] {level: e2e}
def test_every_core_op_and_read(db: gl.Database, path: pathlib.Path) -> None:
    """Every CoreOp through submit, and the core reads, give the Rust results."""
    t = gl.CommitResult
    r = db.submit(
        [
            gl.DeclarePredicate(EX.tag, multi=True),
            gl.DeclareHedgeType(EX.Meeting, roles=[gl.Role(EX.agent), gl.Role(EX.target)]),
            gl.Insert(EX.alice, EX.knows, EX.bob, graph=EX.episode1),
            gl.Set(EX.alice, EX.age, 41),
            gl.Insert(EX.alice, EX.tag, "x"),
            gl.Insert(EX.alice, EX.tag, "y"),
            gl.CreateGraph(EX.agent),
            gl.Nest(EX.episode1, EX.agent),
            gl.InsertUnasserted(EX.carol, EX.says, EX.dan),
            gl.StatementId(EX.alice, EX.age, 41),
        ],
        durability=Durability.SYNC,
    )
    assert isinstance(r, t) and r.batch > 0
    (_, ty, s_knows, _, s_x, s_y, g_agent, nested, unasserted, s_age) = r.results
    assert isinstance(ty, gl.Id) and ty.kind == "iri"
    assert isinstance(s_knows, gl.Id) and s_knows.kind == "statement"
    assert s_x != s_y and nested is True
    assert isinstance(g_agent, gl.Id) and g_agent.kind == "graph"
    assert isinstance(s_age, gl.Id) and isinstance(unasserted, gl.Id)
    r2 = db.submit(
        [
            gl.Set(EX.alice, EX.age, 42),
            gl.Annotate(s_knows, EX.source, EX.doc1),
            gl.CreateHedge(EX.Meeting, roles=[(EX.agent, EX.dan), (EX.target, s_knows)]),
            gl.AddMember(s_knows, EX.agent),
            gl.MembershipId(s_knows, EX.agent),
            gl.MoveMember(s_knows, EX.agent, EX.archive),
            gl.Unnest(EX.episode1, EX.agent),
        ]
    ).results
    a1, hedge, added, mem_id, moved, unnested = r2[1:]
    assert isinstance(hedge, gl.Id) and hedge.kind == "hyperedge"
    assert added is True and isinstance(mem_id, gl.Id) and isinstance(moved, list)
    assert unnested is True
    r3 = db.submit(
        [
            gl.UpdateRole(hedge, EX.agent, EX.erin),
            gl.RoleId(hedge, EX.agent, EX.erin),
            gl.Update(s_age, 43, expected=42),
            gl.Remove(EX.alice, EX.tag, "x"),
            gl.RemoveMember(s_knows, EX.archive),
            gl.DropGraph(EX.agent, recursive=False),
        ]
    ).results
    assert r3[0] is None and isinstance(r3[1], gl.Id) and r3[2] is None and r3[3] is True
    with db.read() as rd:
        alice = rd.lookup(EX.alice)
        assert alice is not None
        props = {rd.decode(m.p): rd.decode(m.o) for m in rd.props(alice)}
        assert props[EX.age] == 43
        assert rd.stmt(s_knows) == (alice, rd.lookup(EX.knows), rd.lookup(EX.bob))
        assert [m.id for m in rd.inc(rd.lookup(EX.bob))] == [s_knows]
        h = rd.hedge(hedge)
        assert h is not None and h.type == rd.lookup(EX.Meeting)
        assert rd.decode(rd.home_graph(s_knows)) == EX.episode1
        assert rd.lookup(EX.nobody) is None and rd.stmt(gl.Id(0x4FFFFFFFFFFFFFFF)) is None
    d = db.submit([gl.Delete(hedge), gl.Delete(s_knows, gl.DeleteMode.DETACH)]).results
    assert isinstance(d[0], list) and s_knows in d[1] and a1 in d[1]
    assert db.file_size() > 0
    assert db.sparql_update(PFX + "INSERT DATA { :x :y :z }", Durability.ORDERED).inserted == 1
    assert len(list(db.datalog("@prefix : <http://ex.org/> .\n?- triple(:x, :y, Z)."))) == 1
    assert db.sparql(PFX + "ASK { :x :y :z }") is True
    db.admin().sweep(window_ms=0)


# @tg: verifies:: [[openspec:python-binding#Python API parity]] {level: e2e}
def test_interactive_write_methods(db: gl.Database) -> None:
    with db.write(Durability.SYNC) as w:
        s = w.insert(EX.a, EX.knows, EX.b)
        assert s is not None
        w.set(EX.a, EX.age, 30)
        a = w.annotate(s, EX.source, EX.doc)
        assert w.stmt(s) is not None, "reads see the transaction's own writes"
        assert len(w.out(w.lookup(EX.a))) == 2
        w.update(s, EX.c)
        assert w.sparql(PFX + "ASK { :a :knows :c }") is True
        w.delete(a)
        assert w.run(gl.Insert(EX.c, EX.p, 1)) is None, "an inline property has no ID"
    with db.read() as r:
        assert r.decode(r.stmt(s)[2]) == EX.c  # type: ignore[index]


# @tg: verifies:: [[openspec:python-binding#Python transactions as context managers]] {level: unit}
def test_database_context_manager(path: pathlib.Path) -> None:
    with gl.Database.open(path) as db:
        db.submit([gl.Insert(EX.a, EX.p, EX.b)])
    with gl.Database.open(path) as db:
        assert db.sparql(PFX + "ASK { :a :p :b }") is True
    with pytest.raises(gl.TransactionClosedError):
        db.read()
