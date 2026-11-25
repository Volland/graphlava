use graphlava::{Database, Durability, OpenOptions};
fn main() -> graphlava::Result<()> {
    let dir = std::env::temp_dir().join(format!("glv-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open(dir.join("s.glv"), OpenOptions::default())?;
    db.sparql_update(
        r#"PREFIX : <http://ex.org/>
      INSERT DATA { :alice a :Person ; :email "a@x" . :bob a :Person .
                    :alice :worksAt :acme {| :confidence 0.9 |} .
                    :a :knows :b . :b :knows :c . :c :knows :a . }"#,
        Durability::Ordered,
    )?;
    for q in [
        "PREFIX : <http://ex.org/> SELECT ?p ?e WHERE { ?p a :Person OPTIONAL { ?p :email ?e } }",
        "PREFIX : <http://ex.org/> SELECT ?c WHERE { << :alice :worksAt :acme ~ ?r >> . ?r :confidence ?c }",
        "PREFIX : <http://ex.org/> SELECT ?x ?y ?z WHERE { ?x :knows ?y . ?y :knows ?z . ?z :knows ?x }",
        "PREFIX : <http://ex.org/> SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
        "PREFIX : <http://ex.org/> SELECT ?y WHERE { :a :knows+ ?y } ORDER BY ?y",
        "PREFIX : <http://ex.org/> ASK { :a :knows :b }",
    ] {
        println!("{q}\n  => {:?}", db.sparql(q)?);
    }
    println!(
        "{:?}",
        db.datalog(
            r#"@prefix : <http://ex.org/> .
        reach(X,Y) :- triple(X, :knows, Y).
        reach(X,Z) :- reach(X,Y), triple(Y, :knows, Z).
        ?- reach(:a, Y)."#
        )?
    );
    println!("{:?}", db.sparql(r#"PREFIX : <http://ex.org/> SELECT ?y WHERE { SERVICE <meta:datalog> {
        """ @prefix : <http://ex.org/> . reach(?x, ?y) :- triple(?x, :knows, ?y) . reach(?x, ?z) :- reach(?x, ?y), triple(?y, :knows, ?z) . """ :p :o .
        :a :reach ?y } }"#));
    db.close()?;
    Ok(())
}
