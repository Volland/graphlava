//! Scenarios of `specs/hyperedges/spec.md`.

use graphlava::term::RDF_TYPE;
use graphlava::{
    CoreOp, Database, DeleteMode, Durability, Error, HedgeTypeDecl, Id, Literal, Reader, RoleSpec,
    Term, TermRef, WriteRequest,
};

use crate::fixture::*;

fn when() -> Term {
    Term::lit(Literal::date_time("2026-11-24T10:00:00Z"))
}

/// `:Meeting` with roles `:agent`, `:target` (both required) and the literal
/// attribute `:when`.
fn declare_meeting(db: &Database) {
    write(db, |w| {
        w.declare_hedge_type(
            iri("Meeting"),
            HedgeTypeDecl {
                roles: vec![RoleSpec::one(iri("agent")), RoleSpec::one(iri("target"))],
                attrs: vec![iri("when").into()],
            },
        )
        .unwrap()
    });
}

fn meeting_op(agent: &str, target: &str, graph: Option<&str>) -> CoreOp {
    CoreOp::CreateHedge {
        ty: iri("Meeting").into(),
        graph: graph.map(|g| TermRef::from(iri(g))),
        roles: vec![
            (iri("agent").into(), iri(agent).into()),
            (iri("target").into(), iri(target).into()),
        ],
        attrs: vec![(iri("when").into(), when().into())],
    }
}

fn create(db: &Database, op: CoreOp) -> graphlava::Result<Id> {
    db.submit(WriteRequest { ops: vec![op] }, Durability::Ordered)
        .map(|t| match t.results[0] {
            graphlava::OpResult::Id(Some(id)) => id,
            ref r => panic!("{r:?}"),
        })
}

fn id_of_iri(db: &Database, local: &str) -> Id {
    id_of(db, &iri(local)).unwrap()
}

// @tg: verifies:: [[openspec:hyperedges#Typed hyperedges with roles#Create a meeting hyperedge]] {level: e2e}
#[test]
fn create_a_meeting_hyperedge() {
    let f = Fixture::new();
    let db = f.open();
    declare_meeting(&db);
    let e = create(&db, meeting_op("alice", "bob", None)).unwrap();
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let h = r.hedge(e).unwrap().unwrap();
    assert_eq!(h.ty, id_of_iri(&db, "Meeting"));
    assert_eq!(
        h.roles,
        vec![
            (id_of_iri(&db, "agent"), id_of_iri(&db, "alice")),
            (id_of_iri(&db, "target"), id_of_iri(&db, "bob")),
        ]
    );
    let when_p = id_of_iri(&db, "when");
    let attrs = r.out(e, Some(when_p)).unwrap();
    assert_eq!(attrs.len(), 1);
    assert_eq!(r.decode(attrs[0].o).unwrap(), when());
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:hyperedges#Typed hyperedges with roles#Missing role rejected]] {level: e2e}
#[test]
fn missing_role_rejected() {
    let f = Fixture::new();
    let db = f.open();
    declare_meeting(&db);
    let missing = create(
        &db,
        CoreOp::CreateHedge {
            ty: iri("Meeting").into(),
            graph: None,
            roles: vec![(iri("agent").into(), iri("alice").into())],
            attrs: vec![],
        },
    );
    assert!(
        matches!(missing, Err(Error::HyperedgeShapeMismatch(_))),
        "{missing:?}"
    );
    // the other mismatch cases of rule he-type-declared
    for roles in [
        vec![
            (iri("agent"), iri("alice")),
            (iri("target"), iri("bob")),
            (iri("venue"), iri("room1")),
        ],
        vec![
            (iri("agent"), iri("alice")),
            (iri("agent"), iri("carol")),
            (iri("target"), iri("bob")),
        ],
    ] {
        let r = create(
            &db,
            CoreOp::CreateHedge {
                ty: iri("Meeting").into(),
                graph: None,
                roles: roles
                    .into_iter()
                    .map(|(a, b)| (a.into(), b.into()))
                    .collect(),
                attrs: vec![],
            },
        );
        assert!(matches!(r, Err(Error::HyperedgeShapeMismatch(_))), "{r:?}");
    }
    let literal_filler = create(
        &db,
        CoreOp::CreateHedge {
            ty: iri("Meeting").into(),
            graph: None,
            roles: vec![
                (iri("agent").into(), iri("alice").into()),
                (iri("target").into(), Term::string("bob").into()),
            ],
            attrs: vec![],
        },
    );
    assert!(matches!(
        literal_filler,
        Err(Error::HyperedgeShapeMismatch(_))
    ));
    let undeclared = create(
        &db,
        CoreOp::CreateHedge {
            ty: iri("Party").into(),
            graph: None,
            roles: vec![],
            attrs: vec![],
        },
    );
    assert!(matches!(undeclared, Err(Error::HyperedgeShapeMismatch(_))));
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert!(
        r.hedges_of_type(id_of_iri(&db, "Meeting"), None)
            .unwrap()
            .is_empty()
    );
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:hyperedges#Literal attributes of hyperedges#Meeting time is an attribute]] {level: e2e}
#[test]
fn meeting_time_is_an_attribute() {
    let f = Fixture::new();
    let db = f.open();
    declare_meeting(&db);
    let e = create(&db, meeting_op("alice", "bob", None)).unwrap();
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let when_p = id_of_iri(&db, "when");
    let by_value = r
        .find_value(when_p, &Literal::date_time("2026-11-24T10:00:00Z"))
        .unwrap();
    assert_eq!(by_value.len(), 1);
    assert_eq!(by_value[0].s, e);
    let t = by_value[0].o;
    assert!(t.is_inline(), "the time is an inline literal of e");
    assert!(
        r.incidence(t, None, None).unwrap().is_empty(),
        "no incidence entry holds the time"
    );
    let all = r.find(Some(e), None, None).unwrap();
    assert!(all.iter().any(|m| m.p == when_p && m.o == t));
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:hyperedges#Role triples are queryable#Find meetings of an agent]] {level: e2e}
#[test]
fn find_meetings_of_an_agent() {
    let f = Fixture::new();
    let db = f.open();
    declare_meeting(&db);
    let m1 = create(&db, meeting_op("alice", "bob", None)).unwrap();
    let m2 = create(&db, meeting_op("alice", "carol", None)).unwrap();
    let _m3 = create(&db, meeting_op("bob", "alice", None)).unwrap();
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let (agent, alice, meeting) = (
        id_of_iri(&db, "agent"),
        id_of_iri(&db, "alice"),
        id_of_iri(&db, "Meeting"),
    );
    let ty = r.lookup(&Term::iri(RDF_TYPE)).unwrap().unwrap();
    // ?e :agent :alice . ?e a :Meeting — a join of two virtual relations
    let mut found: Vec<Id> = r
        .find(None, Some(agent), Some(alice))
        .unwrap()
        .into_iter()
        .filter(|m| {
            !r.find(Some(m.s), Some(ty), Some(meeting))
                .unwrap()
                .is_empty()
        })
        .map(|m| m.s)
        .collect();
    found.sort();
    assert_eq!(found, vec![m1, m2]);
    // role triples carry no ID until promoted, and nothing is in the orderings
    assert!(
        r.find(None, Some(agent), Some(alice))
            .unwrap()
            .iter()
            .all(|m| m.id.is_none())
    );
    assert!(
        r.scan_ordering(graphlava::Perm::Pos, &[agent])
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert_eq!(r.find(None, Some(ty), Some(meeting)).unwrap().len(), 3);
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:hyperedges#Hyperedges in graphs and type scans#Scope and type together]] {level: e2e}
#[test]
fn scope_and_type_together() {
    let f = Fixture::new();
    let db = f.open();
    declare_meeting(&db);
    let a = create(&db, meeting_op("alice", "bob", Some("g1"))).unwrap();
    let b = create(&db, meeting_op("carol", "dan", Some("g1"))).unwrap();
    let _c = create(&db, meeting_op("alice", "dan", Some("g2"))).unwrap();
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let meeting = id_of_iri(&db, "Meeting");
    let g1 = r.graph(&iri("g1")).unwrap().unwrap();
    assert_eq!(
        r.hedges_of_type(meeting, Some((g1, false))).unwrap(),
        vec![a, b]
    );
    assert_eq!(r.hedges_of_type(meeting, None).unwrap().len(), 3);
    assert_eq!(r.hedge(a).unwrap().unwrap().graph, g1);
    // role triples in a graph scope
    let agent = id_of_iri(&db, "agent");
    assert_eq!(
        r.find_in(g1, false, None, Some(agent), None).unwrap().len(),
        2
    );
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:hyperedges#Annotating a role#Confidence on a role]] {level: e2e}
#[test]
fn confidence_on_a_role() {
    let f = Fixture::new();
    let db = f.open();
    declare_meeting(&db);
    let e = create(&db, meeting_op("alice", "bob", None)).unwrap();
    let rid = write(&db, |w| {
        let rid = w.role_id(e, iri("target"), iri("bob")).unwrap();
        w.annotate(rid, iri("confidence"), Term::double(0.7))
            .unwrap();
        // the statement-ID path names the same promoted edge
        assert_eq!(
            w.statement_id(e, iri("target"), iri("bob")).unwrap(),
            Some(rid)
        );
        rid
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let (target, bob) = (id_of_iri(&db, "target"), id_of_iri(&db, "bob"));
    assert_eq!(r.role_id(e, target, bob).unwrap(), Some(rid));
    assert_eq!(
        r.decode(r.props(rid).unwrap()[0].o).unwrap(),
        Term::double(0.7)
    );
    let h = r.hedge(e).unwrap().unwrap();
    assert_eq!(h.roles.len(), 2, "the hyperedge is otherwise unchanged");
    let m = r.find(Some(e), Some(target), None).unwrap();
    assert_eq!(m[0].id, Some(rid));
    drop(r);
    // changing the filler keeps the promoted ID and its annotation (rule he-promote)
    write(&db, |w| {
        w.update_role(e, iri("target"), iri("carol")).unwrap()
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let carol = id_of_iri(&db, "carol");
    assert_eq!(r.role_id(e, target, carol).unwrap(), Some(rid));
    assert_eq!(r.stmt(rid).unwrap().unwrap().o, carol);
    assert_eq!(r.props(rid).unwrap().len(), 1);
    assert!(r.incidence(bob, Some(target), None).unwrap().is_empty());
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:hyperedges#Hyperedge roles block delete#Hyperedge role blocks delete]] {level: e2e}
// @tg: verifies:: [[openspec:hyperedges#Hyperedge roles block delete#Detach removes the hyperedge]] {level: e2e}
#[test]
fn hyperedge_role_blocks_delete_and_detach_removes_it() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| {
        w.declare_hedge_type(
            iri("Cites"),
            HedgeTypeDecl {
                roles: vec![RoleSpec::one(iri("source")), RoleSpec::one(iri("claim"))],
                attrs: vec![],
            },
        )
        .unwrap()
    });
    let (s42, e) = write(&db, |w| {
        let s42 = w.insert(iri("x"), iri("says"), iri("y")).unwrap().unwrap();
        let e = w
            .create_hedge(
                iri("Cites"),
                None,
                vec![
                    (iri("source").into(), iri("paper").into()),
                    (iri("claim").into(), s42.into()),
                ],
                vec![],
            )
            .unwrap();
        (s42, e)
    });
    let blocked = db.submit(
        WriteRequest {
            ops: vec![CoreOp::Delete {
                id: s42,
                mode: DeleteMode::Cascade,
            }],
        },
        Durability::Ordered,
    );
    assert!(
        matches!(blocked, Err(Error::FillsRole { .. })),
        "{blocked:?}"
    );
    let db = f.reopen(db);
    assert!(
        db.read().unwrap().stmt(s42).unwrap().is_some(),
        "nothing changed"
    );
    assert!(db.read().unwrap().hedge(e).unwrap().is_some());
    let gone = write(&db, |w| w.delete(s42, DeleteMode::Detach).unwrap());
    assert_eq!(gone.len(), 2);
    assert!(gone.contains(&s42) && gone.contains(&e));
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert!(r.stmt(s42).unwrap().is_none());
    assert!(r.hedge(e).unwrap().is_none());
    // the other filler remains, with no incidence left
    let paper = id_of_iri(&db, "paper");
    assert!(r.incidence(paper, None, None).unwrap().is_empty());
    drop(r);
    assert_invariants(&db);
}

/// Rule `he-type-change`: optional roles may be appended while records exist;
/// other changes need a migration.
#[test]
fn type_change_rules() {
    let f = Fixture::new();
    let db = f.open();
    declare_meeting(&db);
    let e = create(&db, meeting_op("alice", "bob", None)).unwrap();
    let decl = |roles: Vec<RoleSpec>| HedgeTypeDecl {
        roles,
        attrs: vec![iri("when").into()],
    };
    let shrink = db.submit(
        WriteRequest {
            ops: vec![CoreOp::DeclareHedgeType {
                ty: iri("Meeting").into(),
                decl: decl(vec![RoleSpec::one(iri("agent"))]),
            }],
        },
        Durability::Ordered,
    );
    assert!(matches!(shrink, Err(Error::Invalid(_))));
    write(&db, |w| {
        w.declare_hedge_type(
            iri("Meeting"),
            decl(vec![
                RoleSpec::one(iri("agent")),
                RoleSpec::one(iri("target")),
                RoleSpec::optional(iri("venue")),
            ]),
        )
        .unwrap()
    });
    let db = f.reopen(db);
    // the old record reads the new slot as null
    assert_eq!(db.read().unwrap().hedge(e).unwrap().unwrap().roles.len(), 2);
    write(&db, |w| {
        w.update_role(e, iri("venue"), iri("room1")).unwrap()
    });
    assert_eq!(db.read().unwrap().hedge(e).unwrap().unwrap().roles.len(), 3);
    assert_invariants(&db);
}

/// Rule `he-incidence-complete` after a random sequence of creates, role
/// updates and deletes.
#[test]
fn incidence_stays_complete() {
    let f = Fixture::new();
    let db = f.open();
    declare_meeting(&db);
    let mut x: u64 = 7;
    let mut next = || {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        x >> 33
    };
    let mut live: Vec<Id> = Vec::new();
    for _ in 0..600 {
        let op = match next() % 4 {
            0 | 1 => meeting_op(
                &format!("n{}", next() % 20),
                &format!("n{}", next() % 20),
                Some(&format!("g{}", next() % 3)),
            ),
            2 if !live.is_empty() => CoreOp::UpdateRole {
                hedge: live[(next() as usize) % live.len()],
                role: iri("target").into(),
                filler: iri(&format!("n{}", next() % 20)).into(),
            },
            _ if !live.is_empty() => {
                let e = live.swap_remove((next() as usize) % live.len());
                CoreOp::Delete {
                    id: e,
                    mode: DeleteMode::Cascade,
                }
            }
            _ => continue,
        };
        let res = db
            .submit(WriteRequest { ops: vec![op] }, Durability::Relaxed)
            .unwrap();
        if let graphlava::OpResult::Id(Some(e)) = res.results[0] {
            live.push(e);
        }
    }
    let db = f.reopen(db);
    assert_eq!(
        db.read()
            .unwrap()
            .hedges_of_type(id_of_iri(&db, "Meeting"), None)
            .unwrap()
            .len(),
        live.len()
    );
    assert_invariants(&db);
}

/// Hyperedges take extra memberships like statements (rule `gr-home-graph`).
#[test]
fn hyperedge_membership() {
    let f = Fixture::new();
    let db = f.open();
    declare_meeting(&db);
    let e = create(&db, meeting_op("alice", "bob", Some("g1"))).unwrap();
    write(&db, |w| {
        assert!(w.add_member(e, iri("g2")).unwrap());
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let meeting = id_of_iri(&db, "Meeting");
    let g2 = r.graph(&iri("g2")).unwrap().unwrap();
    assert_eq!(
        r.hedges_of_type(meeting, Some((g2, true))).unwrap(),
        vec![e]
    );
    drop(r);
    let gone = write(&db, |w| w.drop_graph(iri("g1"), false).unwrap());
    assert!(gone.is_empty(), "still a member of g2");
    assert!(db.read().unwrap().hedge(e).unwrap().is_some());
    let gone = write(&db, |w| w.drop_graph(iri("g2"), false).unwrap());
    assert_eq!(gone, vec![e]);
    assert_invariants(&db);
}

/// Rule `dict-sweep-liveness`: graph names, hyperedge types, roles, fillers
/// and the system IRIs survive a sweep with no safety window.
#[test]
fn sweep_keeps_graph_and_hyperedge_terms() {
    let f = Fixture::new();
    let db = f.open();
    declare_meeting(&db);
    let e = create(&db, meeting_op("alice", "bob", Some("g1"))).unwrap();
    db.admin().sweep(std::time::Duration::ZERO).unwrap();
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let h = r.hedge(e).unwrap().unwrap();
    assert_eq!(r.decode(h.graph).unwrap(), iri("g1"));
    assert_eq!(r.decode(h.ty).unwrap(), iri("Meeting"));
    for (role, n) in &h.roles {
        r.decode(*role).unwrap();
        r.decode(*n).unwrap();
    }
    assert!(r.lookup(&Term::iri(RDF_TYPE)).unwrap().is_some());
    assert!(
        r.lookup(&Term::iri(graphlava::term::META_IN))
            .unwrap()
            .is_some()
    );
    drop(r);
    assert_invariants(&db);
}
