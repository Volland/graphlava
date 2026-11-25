//! Scenarios of `specs/term-identity/spec.md`.

use std::ops::Bound;
use std::time::Duration;

use graphlava::id::{self, Kind};
use graphlava::{DeleteMode, Durability, Error, Literal, Reader, Term, TermRef};

use crate::fixture::*;

// @tg: verifies:: [[openspec:term-identity#Stable term identity#Same term interns to the same ID]] {level: e2e}
#[test]
fn same_term_interns_to_the_same_id() {
    let f = Fixture::new();
    let db = f.open();
    let a = write(&db, |w| w.intern(iri("alice")).unwrap());
    let db = f.reopen(db);
    let b = write(&db, |w| w.intern(iri("alice")).unwrap());
    assert_eq!(a, b);
    let db = f.reopen(db);
    assert_eq!(id_of(&db, &iri("alice")), Some(a));
}

// @tg: verifies:: [[openspec:term-identity#Stable term identity#Deleted IDs are not reused]] {level: e2e}
#[test]
fn deleted_ids_are_not_reused() {
    let f = Fixture::new();
    let db = f.open();
    let mut seen = Vec::new();
    let first = write(&db, |w| {
        w.insert(iri("a"), iri("p"), iri("b")).unwrap().unwrap()
    });
    seen.push(first);
    write(&db, |w| w.delete(first, DeleteMode::Cascade).unwrap());
    let db = f.reopen(db);
    for i in 0..5 {
        let id = write(&db, |w| {
            w.insert(iri("a"), iri("p"), iri(&format!("c{i}")))
                .unwrap()
                .unwrap()
        });
        assert!(!seen.contains(&id), "{id} was allocated before");
        assert!(seen.iter().all(|s| *s < id), "allocation is monotone");
        seen.push(id);
    }
    // Same triple as the deleted one gets a new ID too.
    let again = write(&db, |w| {
        w.insert(iri("a"), iri("p"), iri("b")).unwrap().unwrap()
    });
    assert_ne!(again, first);
    let db = f.reopen(db);
    assert_eq!(db.read().unwrap().stmt(first).unwrap(), None);
}

// @tg: verifies:: [[openspec:term-identity#Stable term identity#Kind is readable from the ID]] {level: e2e}
#[test]
fn kind_is_readable_from_the_id() {
    let f = Fixture::new();
    let db = f.open();
    let (i, b, l, s, lit) = write(&db, |w| {
        let i = w.intern(iri("x")).unwrap();
        let b = w.intern(Term::Blank("n".into())).unwrap();
        let l = w
            .intern(Term::string("a long string that is not inline"))
            .unwrap();
        let s = w.insert(i, iri("p"), b).unwrap().unwrap();
        (i, b, l, s, w.intern(int(5)).unwrap())
    });
    let db = f.reopen(db);
    assert_eq!(i.kind(), Kind::Iri);
    assert_eq!(b.kind(), Kind::Blank);
    assert_eq!(l.kind(), Kind::DictLiteral);
    assert_eq!(s.kind(), Kind::Statement);
    assert_eq!(lit.kind(), Kind::InlineLiteral);
    assert_eq!(id::SYSTEM_GRAPH.kind(), Kind::Graph);
    assert!(db.read().unwrap().stmt(s).unwrap().is_some());
}

// @tg: verifies:: [[openspec:term-identity#Order-preserving inline literals#Integer range filter uses value order]] {level: e2e}
#[test]
fn integer_range_filter_uses_value_order() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| {
        w.insert(iri("a"), iri("n"), int(-5)).unwrap();
        w.insert(iri("b"), iri("n"), int(3)).unwrap();
        w.insert(iri("c"), iri("n"), int(1000)).unwrap();
        w.insert(iri("d"), iri("n"), Term::string("x")).unwrap();
    });
    db.admin().build_value_index(&iri("n")).unwrap();
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let n = r.lookup(&iri("n")).unwrap().unwrap();
    let zero = id::enc_int(0).unwrap();
    let max = id::enc_int(id::INT_MAX).unwrap();
    let (hits, used_index) = r
        .find_range(n, Bound::Excluded(zero), Bound::Included(max))
        .unwrap();
    assert!(
        used_index,
        "served by one propidx key range, without decoding"
    );
    let mut vals: Vec<i64> = hits.iter().map(|m| id::dec_int(m.o)).collect();
    vals.sort();
    assert_eq!(vals, vec![3, 1000]);
}

// @tg: verifies:: [[openspec:term-identity#Order-preserving inline literals#Computed doubles are inline and exact]] {level: e2e}
#[test]
fn computed_doubles_are_inline_and_exact() {
    let f = Fixture::new();
    let db = f.open();
    let vals = [0.1f64, 0.73, 1e-30];
    write(&db, |w| {
        for (i, v) in vals.iter().enumerate() {
            w.insert(iri(&format!("m{i}")), iri("score"), Term::double(*v))
                .unwrap();
        }
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let score = r.lookup(&iri("score")).unwrap().unwrap();
    let all = r.find(None, Some(score), None).unwrap();
    assert_eq!(all.len(), 3);
    for m in &all {
        assert!(
            m.o.is_inline_double(),
            "{} is inline, not in the dictionary",
            m.o
        );
    }
    let half = id::enc_double(0.5).unwrap();
    let pos_inf = id::enc_double(f64::INFINITY).unwrap();
    let (hits, _) = r
        .find_range(score, Bound::Excluded(half), Bound::Included(pos_inf))
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(id::dec_double(hits[0].o).to_bits(), 0.73f64.to_bits());
    for v in vals {
        let id = r.lookup(&Term::double(v)).unwrap().unwrap();
        assert_eq!(id::dec_double(id).to_bits(), v.to_bits(), "bit-for-bit");
    }
}

// @tg: verifies:: [[openspec:term-identity#Order-preserving inline literals#Values outside the inline range stay exact]] {level: e2e}
#[test]
fn values_outside_the_inline_range_stay_exact() {
    let f = Fixture::new();
    let db = f.open();
    let big = (1i128 << 60).to_string();
    write(&db, |w| {
        w.insert(
            iri("a"),
            iri("n"),
            Term::lit(Literal::typed(&big, graphlava::term::xsd("integer"))),
        )
        .unwrap()
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let a = r.lookup(&iri("a")).unwrap().unwrap();
    let props = r.props(a).unwrap();
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].o.kind(), Kind::DictLiteral);
    assert_eq!(
        r.decode(props[0].o).unwrap(),
        Term::lit(Literal::typed(big, graphlava::term::xsd("integer")))
    );
}

// @tg: verifies:: [[openspec:term-identity#Canonical literal forms#Leading zeros are folded]] {level: e2e}
#[test]
fn leading_zeros_are_folded() {
    let f = Fixture::new();
    let db = f.open();
    let x = |l: &str| Term::lit(Literal::typed(l, graphlava::term::xsd("integer")));
    write(&db, |w| {
        w.insert(iri("a"), iri("n"), x("01")).unwrap();
        w.insert(iri("a"), iri("n"), x("1")).unwrap();
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let a = r.lookup(&iri("a")).unwrap().unwrap();
    let props = r.props(a).unwrap();
    assert_eq!(props.len(), 1, "one statement exists");
    assert_eq!(r.decode(props[0].o).unwrap(), x("1"));
}

// @tg: verifies:: [[openspec:term-identity#Canonical literal forms#Plain and xsd:string are one term]] {level: e2e}
#[test]
fn plain_and_xsd_string_are_one_term() {
    let f = Fixture::new();
    let db = f.open();
    let (a, b, c, d) = write(&db, |w| {
        let a = w.intern(Term::string("hi")).unwrap();
        let b = w
            .intern(Term::lit(Literal::typed(
                "hi",
                graphlava::term::xsd("string"),
            )))
            .unwrap();
        let long = "a string too long to be inline";
        let c = w.intern(Term::string(long)).unwrap();
        let d = w
            .intern(Term::lit(Literal::typed(
                long,
                graphlava::term::xsd("string"),
            )))
            .unwrap();
        (a, b, c, d)
    });
    assert_eq!(a, b);
    assert_eq!(c, d);
    let db = f.reopen(db);
    assert_eq!(id_of(&db, &Term::string("hi")), Some(a));
}

fn long_text(tag: &str) -> Term {
    Term::string(format!(
        "{tag}: {}",
        "lorem ipsum dolor sit amet ".repeat(80)
    ))
}

// @tg: verifies:: [[openspec:term-identity#Dictionary reclamation without write cost#Unreferenced term is reclaimed]] {level: e2e}
#[test]
fn unreferenced_term_is_reclaimed() {
    let f = Fixture::new();
    let db = f.open();
    let text = Term::string("a medium literal, interned, not owned ...");
    let id = write(&db, |w| {
        w.insert(iri("a"), iri("note"), text.clone()).unwrap();
        w.intern(text.clone()).unwrap()
    });
    assert_eq!(id.kind(), Kind::DictLiteral);
    assert!(!id.is_owned_literal());
    write(&db, |w| {
        w.remove(iri("a"), iri("note"), text.clone(), DeleteMode::Cascade)
            .unwrap()
    });
    let db = f.reopen(db);
    assert_eq!(
        id_of(&db, &text),
        Some(id),
        "still interned before the sweep"
    );
    let rep = db.admin().sweep(Duration::ZERO).unwrap();
    assert!(rep.deleted >= 1, "{rep:?}");
    let db = f.reopen(db);
    assert_eq!(id_of(&db, &text), None, "gone from the dictionary");
    assert!(matches!(
        db.read().unwrap().decode(id),
        Err(Error::NotFound(_))
    ));
}

// @tg: verifies:: [[openspec:term-identity#Dictionary reclamation without write cost#Overwritten long text is freed at once]] {level: e2e}
#[test]
fn overwritten_long_text_is_freed_at_once() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| {
        w.set(iri("doc"), iri("summary"), long_text("v1")).unwrap()
    });
    let old = {
        let r = db.read().unwrap();
        let doc = r.lookup(&iri("doc")).unwrap().unwrap();
        r.props(doc).unwrap()[0].o
    };
    assert!(old.is_owned_literal(), "2 KB text is an owned literal");
    write(&db, |w| {
        w.set(iri("doc"), iri("summary"), long_text("v2")).unwrap()
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert!(
        matches!(r.decode(old), Err(Error::NotFound(_))),
        "freed by the overwrite, without a sweep"
    );
    let doc = r.lookup(&iri("doc")).unwrap().unwrap();
    assert_eq!(
        r.decode(r.props(doc).unwrap()[0].o).unwrap(),
        long_text("v2")
    );
}

// @tg: verifies:: [[openspec:term-identity#Dictionary reclamation without write cost#Term re-referenced during a sweep survives]] {level: e2e}
#[test]
fn term_re_referenced_during_a_sweep_survives() {
    let f = Fixture::new();
    let db = f.open();
    let text = Term::string("re-referenced between mark and sweep");
    write(&db, |w| {
        w.insert(iri("a"), iri("note"), text.clone()).unwrap()
    });
    write(&db, |w| {
        w.remove(iri("a"), iri("note"), text.clone(), DeleteMode::Cascade)
            .unwrap()
    });
    let rep = db
        .admin()
        .sweep_hooked(Duration::ZERO, || {
            // Unreferenced at the mark snapshot; a writer references it again.
            let mut w = db.write(Durability::Ordered).unwrap();
            w.insert(iri("b"), iri("note"), text.clone()).unwrap();
            w.commit().unwrap();
        })
        .unwrap();
    assert!(rep.kept_rereferenced >= 1, "{rep:?}");
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let id = r.lookup(&text).unwrap().expect("kept");
    let b = r.lookup(&iri("b")).unwrap().unwrap();
    assert_eq!(r.props(b).unwrap()[0].o, id);
    assert_eq!(r.decode(id).unwrap(), text);
}

// @tg: verifies:: [[openspec:term-identity#Dictionary reclamation without write cost#Recently interned term survives a sweep]] {level: e2e}
#[test]
fn recently_interned_term_survives_a_sweep() {
    let f = Fixture::new();
    let db = f.open();
    let text = Term::string("interned a moment ago, no references");
    let id = write(&db, |w| w.intern(TermRef::Term(text.clone())).unwrap());
    let rep = db.admin().sweep(Duration::from_secs(3600)).unwrap();
    assert!(rep.kept_young >= 1, "{rep:?}");
    let db = f.reopen(db);
    assert_eq!(id_of(&db, &text), Some(id));
}
