//! Scenarios of `specs/statement-store/spec.md`.

use graphlava::{
    CoreOp, DeleteMode, Durability, Error, Literal, PredicateDecl, Reader, Term, TermRef,
    WriteRequest,
};

use crate::fixture::*;

// @tg: verifies:: [[openspec:statement-store#Statement identity under update#Overwrite keeps ID and annotations]] {level: e2e}
#[test]
fn overwrite_keeps_id_and_annotations() {
    let f = Fixture::new();
    let db = f.open();
    let id = write(&db, |w| {
        let id = w
            .insert(iri("alice"), iri("worksAt"), iri("acme"))
            .unwrap()
            .unwrap();
        w.annotate(id, iri("confidence"), Term::double(0.9))
            .unwrap();
        id
    });
    write(&db, |w| w.update(id, iri("globex"), None).unwrap());
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let t = r.stmt(id).unwrap().unwrap();
    assert_eq!(r.decode(t.o).unwrap(), iri("globex"));
    let ann = r.props(id).unwrap();
    assert_eq!(ann.len(), 1);
    assert_eq!(r.decode(ann[0].o).unwrap(), Term::double(0.9));
    let (a, w, acme) = (
        r.lookup(&iri("alice")).unwrap().unwrap(),
        r.lookup(&iri("worksAt")).unwrap().unwrap(),
        r.lookup(&iri("acme")).unwrap().unwrap(),
    );
    assert!(r.find(Some(a), Some(w), Some(acme)).unwrap().is_empty());
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:statement-store#Unique statements by default#Repeated insert is idempotent]] {level: e2e}
#[test]
fn repeated_insert_is_idempotent() {
    let f = Fixture::new();
    let db = f.open();
    let a = write(&db, |w| w.insert(iri("a"), iri("knows"), iri("b")).unwrap());
    let b = write(&db, |w| w.insert(iri("a"), iri("knows"), iri("b")).unwrap());
    assert_eq!(a, b);
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let knows = r.lookup(&iri("knows")).unwrap().unwrap();
    assert_eq!(r.find(None, Some(knows), None).unwrap().len(), 1);
}

// @tg: verifies:: [[openspec:statement-store#Unique statements by default#Multi predicate keeps parallel statements]] {level: e2e}
#[test]
fn multi_predicate_keeps_parallel_statements() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| {
        w.declare_predicate(
            iri("transfer"),
            PredicateDecl {
                multi: true,
                functional: false,
            },
        )
        .unwrap()
    });
    let a = write(&db, |w| {
        w.insert(iri("a"), iri("transfer"), iri("b"))
            .unwrap()
            .unwrap()
    });
    let b = write(&db, |w| {
        w.insert(iri("a"), iri("transfer"), iri("b"))
            .unwrap()
            .unwrap()
    });
    assert_ne!(a, b);
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let t = r.lookup(&iri("transfer")).unwrap().unwrap();
    assert_eq!(r.find(None, Some(t), None).unwrap().len(), 2);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:statement-store#Functional predicates update in place#Set on a functional predicate keeps annotations]] {level: e2e}
#[test]
fn set_on_a_functional_predicate_keeps_annotations() {
    let f = Fixture::new();
    let db = f.open();
    let id = write(&db, |w| {
        w.declare_predicate(
            iri("currentLocation"),
            PredicateDecl {
                multi: false,
                functional: true,
            },
        )
        .unwrap();
        let id = w
            .insert(iri("alice"), iri("currentLocation"), iri("paris"))
            .unwrap()
            .unwrap();
        w.annotate(id, iri("source"), iri("gps")).unwrap();
        id
    });
    let same = write(&db, |w| {
        w.insert(iri("alice"), iri("currentLocation"), iri("rome"))
            .unwrap()
    });
    assert_eq!(same, Some(id));
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert_eq!(
        r.decode(r.stmt(id).unwrap().unwrap().o).unwrap(),
        iri("rome")
    );
    let src = r.lookup(&iri("source")).unwrap().unwrap();
    assert_eq!(r.find(Some(id), Some(src), None).unwrap().len(), 1);
    // Declaring functional over a subject with two objects is rejected.
    drop(r);
    let err = write(&db, |w| {
        w.insert(iri("bob"), iri("likes"), iri("x")).unwrap();
        w.insert(iri("bob"), iri("likes"), iri("y")).unwrap();
        w.declare_predicate(
            iri("likes"),
            PredicateDecl {
                multi: false,
                functional: true,
            },
        )
    });
    assert!(matches!(err, Err(Error::Conflict(_))), "{err:?}");
}

// @tg: verifies:: [[openspec:statement-store#Functional predicates update in place#Compare-and-set update]] {level: e2e}
#[test]
fn compare_and_set_update() {
    let f = Fixture::new();
    let db = f.open();
    let id = write(&db, |w| {
        w.insert(iri("alice"), iri("loc"), iri("paris"))
            .unwrap()
            .unwrap()
    });
    write(&db, |w| {
        w.update(id, iri("rome"), Some(TermRef::Term(iri("paris"))))
            .unwrap()
    });
    let stale = db.submit(
        WriteRequest {
            ops: vec![CoreOp::Update {
                id,
                o: iri("berlin").into(),
                expected: Some(iri("paris").into()),
            }],
        },
        Durability::Ordered,
    );
    assert!(matches!(stale, Err(Error::Conflict(_))), "{stale:?}");
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert_eq!(
        r.decode(r.stmt(id).unwrap().unwrap().o).unwrap(),
        iri("rome")
    );
}

// @tg: verifies:: [[openspec:statement-store#Inline literal properties#Literal property is queryable as a triple]] {level: e2e}
#[test]
fn literal_property_is_queryable_as_a_triple() {
    let f = Fixture::new();
    let db = f.open();
    let none = write(&db, |w| {
        w.insert(iri("alice"), iri("age"), int(42)).unwrap()
    });
    assert_eq!(none, None, "an inline property has no ID until promoted");
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let age = r.lookup(&iri("age")).unwrap().unwrap();
    let rows = r.find(None, Some(age), None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(r.decode(rows[0].s).unwrap(), iri("alice"));
    assert_eq!(r.decode(rows[0].o).unwrap(), int(42));
}

// @tg: verifies:: [[openspec:statement-store#Inline literal properties#Literal predicate scan reads only nodes that have the predicate]] {level: e2e}
#[test]
fn literal_predicate_scan_reads_only_nodes_that_have_the_predicate() {
    // The record-count check lives in the unit test `store::tests::plist_scan_reads_only_matching_records`;
    // here the same pattern runs end to end on a real file.
    let f = Fixture::new();
    let db = f.open();
    let ops: Vec<CoreOp> = (0..5000)
        .map(|i| CoreOp::Insert {
            s: iri(&format!("n{i}")).into(),
            p: iri("name").into(),
            o: Term::string(format!("n{i}")).into(),
            graph: None,
        })
        .chain((0..100).map(|i| CoreOp::Insert {
            s: iri(&format!("n{i}")).into(),
            p: iri("age").into(),
            o: int(20 + i).into(),
            graph: None,
        }))
        .collect();
    db.submit(WriteRequest { ops }, Durability::Ordered)
        .unwrap();
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let age = r.lookup(&iri("age")).unwrap().unwrap();
    assert_eq!(r.find(None, Some(age), None).unwrap().len(), 100);
}

// @tg: verifies:: [[openspec:statement-store#Inline literal properties#Value lookup works without a declared index]] {level: e2e}
#[test]
fn value_lookup_works_without_a_declared_index() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| {
        w.insert(iri("a"), iri("email"), Term::string("a@b.c"))
            .unwrap();
        w.insert(iri("b"), iri("email"), Term::string("x@y.z"))
            .unwrap();
        w.insert(iri("c"), iri("email"), Term::string("a@b.c"))
            .unwrap();
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let email = r.lookup(&iri("email")).unwrap().unwrap();
    let mut subjects: Vec<Term> = r
        .find_value(email, &Literal::string("a@b.c"))
        .unwrap()
        .iter()
        .map(|m| r.decode(m.s).unwrap())
        .collect();
    subjects.sort_by_key(|t| t.to_string());
    assert_eq!(subjects, vec![iri("a"), iri("c")]);
}

// @tg: verifies:: [[openspec:statement-store#Promotion on annotation#Annotating an inline property]] {level: e2e}
#[test]
fn annotating_an_inline_property() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| {
        w.insert(iri("alice"), iri("age"), int(42)).unwrap()
    });
    let id = write(&db, |w| {
        let id = w
            .statement_id(iri("alice"), iri("age"), int(42))
            .unwrap()
            .unwrap();
        w.annotate(id, iri("source"), iri("doc5")).unwrap();
        id
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let src = r.lookup(&iri("source")).unwrap().unwrap();
    let ann = r.find(Some(id), Some(src), None).unwrap();
    assert_eq!(ann.len(), 1);
    assert_eq!(r.decode(ann[0].o).unwrap(), iri("doc5"));
    // Still one `:age` value, now with its ID.
    let alice = r.lookup(&iri("alice")).unwrap().unwrap();
    let props = r.props(alice).unwrap();
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].id, Some(id));
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:statement-store#Promotion on annotation#Promoted property keeps ID on overwrite]] {level: e2e}
#[test]
fn promoted_property_keeps_id_on_overwrite() {
    let f = Fixture::new();
    let db = f.open();
    let id = write(&db, |w| {
        w.insert(iri("alice"), iri("age"), int(42)).unwrap();
        let id = w
            .statement_id(iri("alice"), iri("age"), int(42))
            .unwrap()
            .unwrap();
        w.annotate(id, iri("source"), iri("doc5")).unwrap();
        id
    });
    let kept = write(&db, |w| w.set(iri("alice"), iri("age"), int(43)).unwrap());
    assert_eq!(kept, Some(id));
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert_eq!(r.decode(r.stmt(id).unwrap().unwrap().o).unwrap(), int(43));
    let src = r.lookup(&iri("source")).unwrap().unwrap();
    assert_eq!(r.find(Some(id), Some(src), None).unwrap().len(), 1);
    assert_invariants(&db);
}

fn chain(db: &graphlava::Database) -> (graphlava::Id, graphlava::Id, graphlava::Id) {
    write(db, |w| {
        let a = w
            .insert(iri("alice"), iri("worksAt"), iri("acme"))
            .unwrap()
            .unwrap();
        let b = w.insert(a, iri("source"), iri("doc1")).unwrap().unwrap();
        let c = w.insert(b, iri("checkedBy"), iri("bob")).unwrap().unwrap();
        w.insert(c, iri("confidence"), Term::double(0.5)).unwrap();
        (a, b, c)
    })
}

// @tg: verifies:: [[openspec:statement-store#Cascade delete of annotations#Annotation chain is removed]] {level: e2e}
#[test]
fn annotation_chain_is_removed() {
    let f = Fixture::new();
    let db = f.open();
    let (a, b, c) = chain(&db);
    let gone = write(&db, |w| w.delete(a, DeleteMode::Cascade).unwrap());
    assert_eq!(gone.len(), 3);
    let db = f.reopen(db);
    let r = db.read().unwrap();
    for id in [a, b, c] {
        assert_eq!(r.stmt(id).unwrap(), None);
        assert!(r.props(id).unwrap().is_empty());
    }
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:statement-store#Cascade delete of annotations#Object-position reference blocks delete]] {level: e2e}
#[test]
fn object_position_reference_blocks_delete() {
    let f = Fixture::new();
    let db = f.open();
    let (x, note) = write(&db, |w| {
        let x = w
            .insert(iri("alice"), iri("worksAt"), iri("acme"))
            .unwrap()
            .unwrap();
        let note = w.insert(iri("note"), iri("about"), x).unwrap().unwrap();
        (x, note)
    });
    let blocked = db.submit(
        WriteRequest {
            ops: vec![CoreOp::Delete {
                id: x,
                mode: DeleteMode::Cascade,
            }],
        },
        Durability::Ordered,
    );
    assert!(
        matches!(blocked, Err(Error::ReferencedByStatement { .. })),
        "{blocked:?}"
    );
    let db = f.reopen(db);
    assert!(
        db.read().unwrap().stmt(x).unwrap().is_some(),
        "nothing changed"
    );
    let gone = write(&db, |w| w.delete(x, DeleteMode::Detach).unwrap());
    assert!(gone.contains(&x) && gone.contains(&note));
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert_eq!(r.stmt(x).unwrap(), None);
    assert_eq!(r.stmt(note).unwrap(), None);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:statement-store#Cascade delete of annotations#Keep annotations on request]] {level: e2e}
#[test]
fn keep_annotations_on_request() {
    let f = Fixture::new();
    let db = f.open();
    let (a, b, c) = chain(&db);
    write(&db, |w| w.delete(a, DeleteMode::KeepAnnotations).unwrap());
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert_eq!(r.stmt(a).unwrap(), None);
    assert_eq!(
        r.stmt(b).unwrap().unwrap().s,
        a,
        "subject resolves to no statement"
    );
    assert!(r.stmt(c).unwrap().is_some());
}

// @tg: verifies:: [[openspec:statement-store#Index coverage of base orderings#Object-bound lookup]] {level: e2e}
#[test]
fn object_bound_lookup() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| {
        w.insert(iri("alice"), iri("worksAt"), iri("acme")).unwrap();
        w.insert(iri("bob"), iri("owns"), iri("acme")).unwrap();
        w.insert(iri("bob"), iri("worksAt"), iri("globex")).unwrap();
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let acme = r.lookup(&iri("acme")).unwrap().unwrap();
    // `inc` is an OSP prefix range keyed by :acme
    let rows = r.inc(acme, None).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|m| m.o == acme));
    let osp = r
        .scan_ordering(graphlava::Perm::Osp, &[acme])
        .unwrap()
        .unwrap();
    assert_eq!(osp.len(), 2);
}

#[test]
fn on_demand_ordering_builds_online() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| {
        for i in 0..50 {
            w.insert(iri(&format!("s{i}")), iri("p"), iri(&format!("o{}", i % 7)))
                .unwrap();
        }
    });
    db.admin().build_ordering(graphlava::Perm::Pso).unwrap();
    write(&db, |w| w.insert(iri("late"), iri("p"), iri("o1")).unwrap());
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let p = r.lookup(&iri("p")).unwrap().unwrap();
    assert_eq!(
        r.scan_ordering(graphlava::Perm::Pso, &[p])
            .unwrap()
            .unwrap()
            .len(),
        51
    );
    assert_invariants(&db);
}

#[test]
fn spilled_property_records_round_trip() {
    let f = Fixture::new();
    let db = f.open();
    let ops = (0..300)
        .map(|i| CoreOp::Insert {
            s: iri("big").into(),
            p: iri(&format!("p{i}")).into(),
            o: int(i).into(),
            graph: None,
        })
        .collect();
    db.submit(WriteRequest { ops }, Durability::Ordered)
        .unwrap();
    write(&db, |w| w.set(iri("big"), iri("p7"), int(-7)).unwrap());
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let big = r.lookup(&iri("big")).unwrap().unwrap();
    let props = r.props(big).unwrap();
    assert_eq!(props.len(), 300);
    let p7 = r.lookup(&iri("p7")).unwrap().unwrap();
    assert_eq!(
        r.find(Some(big), Some(p7), None).unwrap()[0].o,
        r.lookup(&int(-7)).unwrap().unwrap()
    );
    assert_invariants(&db);
}
