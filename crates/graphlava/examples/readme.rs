//! Every snippet of `crates/graphlava/README.md`, run end to end.
//! `cargo run -p graphlava --example readme`

use graphlava::{
    CoreOp, Database, DeleteMode, Durability, HedgeTypeDecl, OpenOptions, Reader, RoleSpec, Term,
    WriteRequest,
};

fn ex(local: &str) -> Term {
    Term::iri(format!("http://example.org/{local}"))
}

fn main() -> graphlava::Result<()> {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("memory.glv");

    // --- open and write -------------------------------------------------------
    let db = Database::open(&path, OpenOptions::default())?;
    let mut w = db.write(Durability::Ordered)?;
    let works = w
        .insert(ex("alice"), ex("worksAt"), ex("acme"))?
        .expect("a node-valued statement has an ID");
    w.annotate(works, ex("confidence"), Term::double(0.9))?; // a statement about a statement
    w.insert(ex("alice"), ex("age"), Term::integer(42))?; // an inline property
    w.commit()?;

    // update in place: the ID and its annotations stay
    let mut w = db.write(Durability::Ordered)?;
    w.update(works, ex("globex"), None)?;
    w.commit()?;

    // --- read -----------------------------------------------------------------
    let r = db.read()?;
    let t = r.stmt(works)?.expect("still there");
    assert_eq!(r.decode(t.o)?, ex("globex"));
    assert_eq!(
        r.props(works)?.len(),
        1,
        "the annotation followed the update"
    );
    drop(r);

    // --- SPARQL 1.2 -------------------------------------------------------------
    db.sparql_update(
        r#"PREFIX : <http://example.org/>
           INSERT DATA { :bob :worksAt :acme {| :source :hr-export |} }"#,
        Durability::Ordered,
    )?;
    let rows = db.sparql(
        r#"PREFIX : <http://example.org/>
           SELECT ?who ?c WHERE { << ?who :worksAt ?org ~ ?r >> . ?r :confidence ?c }"#,
    )?;
    assert_eq!(rows.rows().len(), 1);

    // --- Datalog ------------------------------------------------------------------
    db.sparql_update(
        "PREFIX : <http://example.org/> INSERT DATA { :a :knows :b . :b :knows :c }",
        Durability::Ordered,
    )?;
    let reach = db.datalog(
        r#"@prefix : <http://example.org/> .
           reach(?x, ?y) :- :knows(?x, ?y) .
           reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
           ?- reach(:a, ?who) ."#,
    )?;
    assert_eq!(reach.rows().len(), 2);

    // --- nested graphs --------------------------------------------------------------
    let mut w = db.write(Durability::Ordered)?;
    w.nest(ex("episode1"), ex("agent"))?;
    w.insert_in(
        ex("episode1"),
        ex("alice"),
        ex("said"),
        Term::string("hello"),
    )?;
    w.commit()?;
    let said = db.sparql(
        r#"PREFIX : <http://example.org/>
           SELECT ?m WHERE { GRAPH :agent { :alice :said ?m } }"#,
    )?;
    assert_eq!(said.rows().len(), 1, "GRAPH sees nested graphs");
    let direct = db.sparql(
        r#"PREFIX : <http://example.org/>
           SELECT ?m WHERE { GRAPH! :agent { :alice :said ?m } }"#,
    )?;
    assert!(direct.rows().is_empty(), "GRAPH! sees only direct members");

    // --- hyperedges -----------------------------------------------------------------
    let mut w = db.write(Durability::Ordered)?;
    w.declare_hedge_type(
        ex("Meeting"),
        HedgeTypeDecl {
            roles: vec![RoleSpec::one(ex("agent")), RoleSpec::one(ex("target"))],
            attrs: vec![ex("when").into()],
        },
    )?;
    let meeting = w.create_hedge(
        ex("Meeting"),
        None,
        vec![
            (ex("agent").into(), ex("alice").into()),
            (ex("target").into(), ex("bob").into()),
        ],
        vec![(
            ex("when").into(),
            Term::lit(graphlava::Literal::date_time("2026-11-24T10:00:00Z")).into(),
        )],
    )?;
    w.commit()?;
    assert_eq!(
        db.read()?.hedge(meeting)?.expect("stored once").roles.len(),
        2
    );

    // --- one-shot requests and durability --------------------------------------------
    let ticket = db.submit(
        WriteRequest {
            ops: vec![CoreOp::Insert {
                s: ex("carol").into(),
                p: ex("knows").into(),
                o: ex("alice").into(),
                graph: None,
            }],
        },
        Durability::Sync,
    )?;
    let graphlava::OpResult::Id(Some(id)) = ticket.results[0] else {
        unreachable!()
    };
    db.submit(
        WriteRequest {
            ops: vec![CoreOp::Delete {
                id,
                mode: DeleteMode::Cascade,
            }],
        },
        Durability::Relaxed,
    )?;

    db.close()?;
    println!("README examples ran");
    Ok(())
}
