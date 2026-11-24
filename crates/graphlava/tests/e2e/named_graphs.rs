//! Scenarios of `specs/named-graphs/spec.md`.

use graphlava::id::DEFAULT_GRAPH;
use graphlava::term::META_IN;
use graphlava::{CoreOp, Database, Durability, Error, Id, Reader, Term, TermRef, WriteRequest};

use crate::fixture::*;

fn gid(db: &Database, name: &str) -> Id {
    db.read()
        .unwrap()
        .graph(&iri(name))
        .unwrap()
        .unwrap_or_else(|| panic!("no graph {name}"))
}

fn in_id(db: &Database) -> Id {
    db.read()
        .unwrap()
        .lookup(&Term::iri(META_IN))
        .unwrap()
        .unwrap()
}

fn submit(db: &Database, ops: Vec<CoreOp>) -> graphlava::Result<Vec<graphlava::OpResult>> {
    db.submit(WriteRequest { ops }, Durability::Ordered)
        .map(|t| t.results)
}

/// Statement IDs of the scope of `g`.
fn scope(db: &Database, g: Id, direct: bool) -> Vec<Id> {
    db.read().unwrap().scope_statements(g, direct).unwrap()
}

// @tg: verifies:: [[openspec:named-graphs#Home graph membership#Statement in its home graph]] {level: e2e}
#[test]
fn statement_in_its_home_graph() {
    let f = Fixture::new();
    let db = f.open();
    let id = write(&db, |w| {
        w.insert_in(iri("g1"), iri("alice"), iri("knows"), iri("bob"))
            .unwrap()
            .unwrap()
    });
    let db = f.reopen(db);
    let g1 = gid(&db, "g1");
    let r = db.read().unwrap();
    assert_eq!(r.home_graph(id).unwrap(), g1);
    let m = r.find(Some(id), Some(in_id(&db)), None).unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].o, g1);
    assert_eq!(m[0].id, None, "a membership has no ID until annotated");
    assert_eq!(r.decode(g1).unwrap(), iri("g1"));
    // the same `:in` triple from the graph's side
    let members = r.find(None, Some(in_id(&db)), Some(g1)).unwrap();
    assert!(members.iter().any(|m| m.s == id));
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:named-graphs#Home graph membership#Extra membership]] {level: e2e}
#[test]
fn extra_membership() {
    let f = Fixture::new();
    let db = f.open();
    let id = write(&db, |w| {
        let id = w
            .insert_in(iri("g1"), iri("alice"), iri("knows"), iri("bob"))
            .unwrap()
            .unwrap();
        assert!(w.add_member(id, iri("g2")).unwrap());
        assert!(!w.add_member(id, iri("g2")).unwrap(), "already a member");
        id
    });
    let db = f.reopen(db);
    let (g1, g2) = (gid(&db, "g1"), gid(&db, "g2"));
    assert_eq!(scope(&db, g1, true), vec![id]);
    assert_eq!(scope(&db, g2, true), vec![id]);
    let r = db.read().unwrap();
    let mut gs = r.graphs_of(id).unwrap();
    gs.sort();
    let mut want = vec![g1, g2];
    want.sort();
    assert_eq!(gs, want);
    assert_eq!(
        r.find_in(
            g2,
            true,
            None,
            None,
            Some(r.lookup(&iri("bob")).unwrap().unwrap())
        )
        .unwrap()
        .len(),
        1
    );
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:named-graphs#One statement in several graphs#Insert into a second graph adds a membership]] {level: e2e}
#[test]
fn insert_into_second_graph_adds_membership() {
    let f = Fixture::new();
    let db = f.open();
    let (a, b) = write(&db, |w| {
        let a = w
            .insert_in(iri("ep1"), iri("alice"), iri("knows"), iri("bob"))
            .unwrap()
            .unwrap();
        let b = w
            .insert_in(iri("ep2"), iri("alice"), iri("knows"), iri("bob"))
            .unwrap()
            .unwrap();
        (a, b)
    });
    assert_eq!(a, b, "no new statement");
    let db = f.reopen(db);
    assert_eq!(scope(&db, gid(&db, "ep1"), true), vec![a]);
    assert_eq!(scope(&db, gid(&db, "ep2"), true), vec![a]);
    let r = db.read().unwrap();
    assert_eq!(
        r.find(None, None, None)
            .unwrap()
            .iter()
            .filter(|m| m.id == Some(a))
            .count(),
        1
    );
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:named-graphs#One statement in several graphs#Delete in one graph keeps the other]] {level: e2e}
#[test]
fn delete_in_one_graph_keeps_the_other() {
    let f = Fixture::new();
    let db = f.open();
    let id = write(&db, |w| {
        let id = w
            .insert_in(iri("ep1"), iri("alice"), iri("knows"), iri("bob"))
            .unwrap()
            .unwrap();
        w.insert_in(iri("ep2"), iri("alice"), iri("knows"), iri("bob"))
            .unwrap();
        w.annotate(id, iri("confidence"), Term::double(0.8))
            .unwrap();
        id
    });
    write(&db, |w| {
        assert!(
            w.remove_in(iri("ep1"), iri("alice"), iri("knows"), iri("bob"))
                .unwrap()
        );
        // not a member of ep1 any more
        assert!(
            !w.remove_in(iri("ep1"), iri("alice"), iri("knows"), iri("bob"))
                .unwrap()
        );
    });
    let db = f.reopen(db);
    let (ep1, ep2) = (gid(&db, "ep1"), gid(&db, "ep2"));
    let r = db.read().unwrap();
    assert!(r.stmt(id).unwrap().is_some());
    assert_eq!(r.props(id).unwrap().len(), 1, "annotations remain");
    assert_eq!(r.graphs_of(id).unwrap(), vec![ep2]);
    drop(r);
    assert!(scope(&db, ep1, true).is_empty());
    assert_eq!(scope(&db, ep2, true), vec![id]);
    // removing the last membership deletes the statement
    write(&db, |w| {
        assert!(
            w.remove_in(iri("ep2"), iri("alice"), iri("knows"), iri("bob"))
                .unwrap()
        )
    });
    let db = f.reopen(db);
    assert!(db.read().unwrap().stmt(id).unwrap().is_none());
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:named-graphs#One statement in several graphs#Per-graph metadata on memberships]] {level: e2e}
#[test]
fn per_graph_metadata_on_memberships() {
    let f = Fixture::new();
    let db = f.open();
    let (m1, m2) = write(&db, |w| {
        let id = w
            .insert_in(iri("ep1"), iri("alice"), iri("knows"), iri("bob"))
            .unwrap()
            .unwrap();
        w.insert_in(iri("ep2"), iri("alice"), iri("knows"), iri("bob"))
            .unwrap();
        // << #42 :in :ep1 >> through the statement-ID path, << #42 :in :ep2 >> directly
        let m1 = w
            .statement_id(id, Term::iri(META_IN), iri("ep1"))
            .unwrap()
            .unwrap();
        let m2 = w.membership_id(id, iri("ep2")).unwrap();
        w.annotate(m1, iri("confidence"), Term::double(0.9))
            .unwrap();
        w.annotate(m2, iri("confidence"), Term::double(0.6))
            .unwrap();
        assert_eq!(w.membership_id(id, iri("ep1")).unwrap(), m1, "stable");
        (m1, m2)
    });
    assert_ne!(m1, m2);
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let conf = |m: Id| r.decode(r.props(m).unwrap()[0].o).unwrap();
    assert_eq!(conf(m1), Term::double(0.9));
    assert_eq!(conf(m2), Term::double(0.6));
    let t = r.stmt(m1).unwrap().unwrap();
    assert_eq!(t.o, gid(&db, "ep1"));
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:named-graphs#Transitive graph scope by default#Parent scope includes children]] {level: e2e}
// @tg: verifies:: [[openspec:named-graphs#Transitive graph scope by default#Direct scope excludes children]] {level: e2e}
#[test]
fn parent_scope_includes_children_direct_excludes() {
    let f = Fixture::new();
    let db = f.open();
    let s = write(&db, |w| {
        w.nest(iri("episode1"), iri("agent")).unwrap();
        w.insert_in(iri("episode1"), iri("alice"), iri("met"), iri("bob"))
            .unwrap()
            .unwrap()
    });
    let db = f.reopen(db);
    let agent = gid(&db, "agent");
    assert_eq!(scope(&db, agent, false), vec![s]);
    assert!(scope(&db, agent, true).is_empty());
    let r = db.read().unwrap();
    let met = r.lookup(&iri("met")).unwrap();
    assert_eq!(r.find_in(agent, false, None, met, None).unwrap().len(), 1);
    assert!(r.find_in(agent, true, None, met, None).unwrap().is_empty());
    // `GRAPH ?g` binds the direct graph, never an ancestor (rule gr-scope-unbound)
    assert_eq!(r.graphs_of(s).unwrap(), vec![gid(&db, "episode1")]);
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:named-graphs#Transitive graph scope by default#Shared child counts once]] {level: e2e}
#[test]
fn shared_child_counts_once() {
    let f = Fixture::new();
    let db = f.open();
    let ids = write(&db, |w| {
        w.nest(iri("episode1"), iri("agent")).unwrap();
        w.nest(iri("episode1"), iri("project")).unwrap();
        w.nest(iri("project"), iri("agent")).unwrap();
        (0..3)
            .map(|i| {
                w.insert_in(iri("episode1"), iri(&format!("s{i}")), iri("p"), iri("o"))
                    .unwrap()
                    .unwrap()
            })
            .collect::<Vec<_>>()
    });
    let db = f.reopen(db);
    let agent = gid(&db, "agent");
    assert_eq!(scope(&db, agent, false), ids);
    let r = db.read().unwrap();
    let p = r.lookup(&iri("p")).unwrap();
    assert_eq!(r.find_in(agent, false, None, p, None).unwrap().len(), 3);
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:named-graphs#Acyclic nesting#Cycle rejected]] {level: e2e}
#[test]
fn cycle_rejected() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| w.nest(iri("a"), iri("b")).unwrap());
    let r = submit(
        &db,
        vec![CoreOp::Nest {
            child: iri("b").into(),
            parent: iri("a").into(),
        }],
    );
    assert!(matches!(r, Err(Error::GraphCycle { .. })), "{r:?}");
    let self_loop = submit(
        &db,
        vec![CoreOp::Nest {
            child: iri("a").into(),
            parent: iri("a").into(),
        }],
    );
    assert!(matches!(self_loop, Err(Error::GraphCycle { .. })));
    let db = f.reopen(db);
    let (a, b) = (gid(&db, "a"), gid(&db, "b"));
    let r = db.read().unwrap();
    assert_eq!(r.parents(a).unwrap(), vec![b]);
    assert!(r.parents(b).unwrap().is_empty(), "nesting unchanged");
    drop(r);
    assert_invariants(&db);
}

/// Rule `gr-dag-depth`: a chain longer than the limit is rejected.
#[test]
fn depth_limit() {
    let f = Fixture::with(graphlava::OpenOptions {
        graph_depth_limit: 4,
        ..Default::default()
    });
    let db = f.open();
    let ops = (0..4)
        .map(|i| CoreOp::Nest {
            child: iri(&format!("c{}", i + 1)).into(),
            parent: iri(&format!("c{i}")).into(),
        })
        .collect();
    submit(&db, ops).unwrap();
    let r = submit(
        &db,
        vec![CoreOp::Nest {
            child: iri("c5").into(),
            parent: iri("c4").into(),
        }],
    );
    assert!(
        matches!(r, Err(Error::GraphDepthExceeded { limit: 4 })),
        "{r:?}"
    );
    // a short branch elsewhere is still fine
    submit(
        &db,
        vec![CoreOp::Nest {
            child: iri("x").into(),
            parent: iri("c1").into(),
        }],
    )
    .unwrap();
    assert_invariants(&db);
}

/// Task 2.3 and rule `gr-in-star-closure`: 40 stacked diamonds (2^40 paths
/// from the bottom to the top) keep `in_star` a set equal to the closure, on
/// edge insert and on edge removal with ancestor recomputation.
#[test]
fn forty_stacked_diamonds() {
    // 40 diamonds are 80 edges deep: above the default limit of 64, so this
    // file is created with a higher one (rule `gr-dag-depth`).
    let f = Fixture::with(graphlava::OpenOptions {
        graph_depth_limit: 128,
        ..Default::default()
    });
    let db = f.open();
    let n = |k: usize| iri(&format!("d{k}"));
    let side = |k: usize, s: &str| iri(&format!("d{k}{s}"));
    let mut ops = Vec::new();
    for k in 0..40 {
        // d{k+1} -> {d{k}l, d{k}r} -> d{k}: the lower node is nested under both sides
        for s in ["l", "r"] {
            ops.push(CoreOp::Nest {
                child: TermRef::from(side(k, s)),
                parent: TermRef::from(n(k)),
            });
            ops.push(CoreOp::Nest {
                child: TermRef::from(n(k + 1)),
                parent: TermRef::from(side(k, s)),
            });
        }
    }
    submit(&db, ops).unwrap();
    let bottom = n(40);
    let id = write(&db, |w| {
        w.insert_in(bottom.clone(), iri("x"), iri("p"), iri("y"))
            .unwrap()
            .unwrap()
    });
    let top = gid(&db, "d0");
    assert_eq!(scope(&db, top, false), vec![id], "the item counts once");
    let r = db.read().unwrap();
    assert_eq!(r.ancestors(gid(&db, "d40")).unwrap().len(), 40 + 80);
    drop(r);
    assert_invariants(&db);
    // Removing one side of the top diamond keeps every ancestor (the other side
    // still reaches d0); removing both cuts the top off.
    submit(
        &db,
        vec![CoreOp::Unnest {
            child: side(0, "l").into(),
            parent: n(0).into(),
        }],
    )
    .unwrap();
    assert_invariants(&db);
    assert_eq!(scope(&db, top, false), vec![id]);
    submit(
        &db,
        vec![CoreOp::Unnest {
            child: side(0, "r").into(),
            parent: n(0).into(),
        }],
    )
    .unwrap();
    let db = f.reopen(db);
    assert_invariants(&db);
    assert!(scope(&db, top, false).is_empty());
    assert_eq!(scope(&db, gid(&db, "d1"), false), vec![id]);
}

// @tg: verifies:: [[openspec:named-graphs#Annotatable membership#Annotating membership]] {level: e2e}
#[test]
fn annotating_membership() {
    let f = Fixture::new();
    let db = f.open();
    let (s, m) = write(&db, |w| {
        let s = w
            .insert_in(iri("g1"), iri("alice"), iri("knows"), iri("bob"))
            .unwrap()
            .unwrap();
        let m = w.membership_id(s, iri("g1")).unwrap();
        w.annotate(
            m,
            iri("addedAt"),
            Term::lit(graphlava::Literal::date_time("2026-11-24T10:42:00Z")),
        )
        .unwrap();
        (s, m)
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let g1 = gid(&db, "g1");
    assert_eq!(r.membership(s, g1).unwrap(), Some(m));
    let t = r.stmt(m).unwrap().unwrap();
    assert_eq!((t.s, t.o), (s, g1));
    let v = r.find(Some(s), Some(in_id(&db)), Some(g1)).unwrap();
    assert_eq!(v[0].id, Some(m), "the virtual triple now carries its ID");
    assert_eq!(r.props(m).unwrap().len(), 1);
    drop(r);
    // deleting the membership's only graph deletes the statement, the
    // membership statement and its annotation
    let gone = write(&db, |w| w.remove_member(s, iri("g1")).unwrap());
    assert!(gone.contains(&s) && gone.contains(&m));
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:named-graphs#Graph drop#Plain drop keeps children]] {level: e2e}
#[test]
fn plain_drop_keeps_children() {
    let f = Fixture::new();
    let db = f.open();
    let (own, child) = write(&db, |w| {
        w.nest(iri("episode1"), iri("agent")).unwrap();
        let own = w
            .insert_in(iri("agent"), iri("a"), iri("p"), iri("b"))
            .unwrap()
            .unwrap();
        w.insert_in(iri("agent"), iri("a"), iri("name"), Term::string("A"))
            .unwrap();
        let child = w
            .insert_in(iri("episode1"), iri("c"), iri("p"), iri("d"))
            .unwrap()
            .unwrap();
        (own, child)
    });
    let gone = write(&db, |w| w.drop_graph(iri("agent"), false).unwrap());
    assert!(gone.contains(&own) && !gone.contains(&child));
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert!(r.graph(&iri("agent")).unwrap().is_none());
    assert!(r.stmt(own).unwrap().is_none());
    let ep1 = gid(&db, "episode1");
    assert!(r.parents(ep1).unwrap().is_empty(), "now a root graph");
    assert_eq!(r.scope_statements(ep1, false).unwrap(), vec![child]);
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:named-graphs#Graph drop#Drop keeps statements shared with another graph]] {level: e2e}
#[test]
fn drop_keeps_shared_statements() {
    let f = Fixture::new();
    let db = f.open();
    let id = write(&db, |w| {
        let id = w
            .insert_in(iri("ep1"), iri("alice"), iri("knows"), iri("bob"))
            .unwrap()
            .unwrap();
        w.add_member(id, iri("ep2")).unwrap();
        id
    });
    let ep1 = gid(&db, "ep1");
    let gone = write(&db, |w| w.drop_graph(iri("ep1"), false).unwrap());
    assert!(gone.is_empty());
    let db = f.reopen(db);
    let ep2 = gid(&db, "ep2");
    let r = db.read().unwrap();
    assert!(r.stmt(id).unwrap().is_some());
    assert_eq!(r.graphs_of(id).unwrap(), vec![ep2]);
    assert_eq!(
        r.home_graph(id).unwrap(),
        ep1,
        "the home graph never changes"
    );
    drop(r);
    assert_eq!(scope(&db, ep2, true), vec![id]);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:named-graphs#Graph drop#Recursive drop]] {level: e2e}
#[test]
fn recursive_drop() {
    let f = Fixture::new();
    let db = f.open();
    let (e1, e2) = write(&db, |w| {
        w.nest(iri("episode1"), iri("agent")).unwrap();
        w.nest(iri("episode2"), iri("agent")).unwrap();
        w.nest(iri("episode2"), iri("other")).unwrap();
        let e1 = w
            .insert_in(iri("episode1"), iri("a"), iri("p"), iri("b"))
            .unwrap()
            .unwrap();
        let e2 = w
            .insert_in(iri("episode2"), iri("c"), iri("p"), iri("d"))
            .unwrap()
            .unwrap();
        (e1, e2)
    });
    let gone = write(&db, |w| w.drop_graph(iri("agent"), true).unwrap());
    assert!(gone.contains(&e1));
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert!(r.graph(&iri("episode1")).unwrap().is_none());
    assert!(r.stmt(e1).unwrap().is_none());
    // episode2 has another surviving parent
    let ep2 = r.graph(&iri("episode2")).unwrap().unwrap();
    assert!(r.stmt(e2).unwrap().is_some());
    assert_eq!(r.parents(ep2).unwrap(), vec![gid(&db, "other")]);
    drop(r);
    assert_invariants(&db);
}

/// Rule `gr-move`: a moved statement keeps its ID, home graph and annotations.
#[test]
fn move_keeps_id_and_annotations() {
    let f = Fixture::new();
    let db = f.open();
    let id = write(&db, |w| {
        let id = w
            .insert_in(iri("inbox"), iri("t"), iri("status"), iri("open"))
            .unwrap()
            .unwrap();
        w.annotate(id, iri("by"), iri("alice")).unwrap();
        w.move_member(id, iri("inbox"), iri("done")).unwrap();
        id
    });
    let db = f.reopen(db);
    let (inbox, done) = (gid(&db, "inbox"), gid(&db, "done"));
    let r = db.read().unwrap();
    assert_eq!(r.graphs_of(id).unwrap(), vec![done]);
    assert_eq!(r.home_graph(id).unwrap(), inbox);
    assert_eq!(r.out(id, r.lookup(&iri("by")).unwrap()).unwrap().len(), 1);
    drop(r);
    // inserting it into its home graph again removes the tombstone
    write(&db, |w| {
        w.insert_in(iri("inbox"), iri("t"), iri("status"), iri("open"))
            .unwrap()
    });
    let mut gs = db.read().unwrap().graphs_of(id).unwrap();
    gs.sort();
    let mut want = vec![inbox, done];
    want.sort();
    assert_eq!(gs, want);
    assert_invariants(&db);
}

/// Inline properties belong to their subject's home graph; a literal written
/// into another graph is a promoted statement of that graph.
#[test]
fn literals_in_named_graphs() {
    let f = Fixture::new();
    let db = f.open();
    let (inline, promoted) = write(&db, |w| {
        let inline = w
            .insert(iri("alice"), iri("age"), Term::integer(30))
            .unwrap();
        let promoted = w
            .insert_in(iri("g1"), iri("alice"), iri("nick"), Term::string("al"))
            .unwrap();
        (inline, promoted)
    });
    assert_eq!(inline, None);
    let promoted = promoted.expect("a literal in a named graph has an ID");
    let db = f.reopen(db);
    let g1 = gid(&db, "g1");
    let r = db.read().unwrap();
    let alice = r.lookup(&iri("alice")).unwrap();
    assert_eq!(
        r.find_in(DEFAULT_GRAPH, true, alice, None, None)
            .unwrap()
            .len(),
        1
    );
    let in_g1 = r.find_in(g1, true, alice, None, None).unwrap();
    assert_eq!(in_g1.len(), 1);
    assert_eq!(in_g1[0].id, Some(promoted));
    drop(r);
    assert_invariants(&db);
}
