use graphlava::{CoreOp, Database, Durability, OpenOptions, Term, WriteRequest};
fn main() -> graphlava::Result<()> {
    let dir = std::env::temp_dir().join(format!("glv-prof-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open(dir.join("p.glv"), OpenOptions::default())?;
    let iri = |s: String| Term::iri(format!("http://ex.org/{s}"));
    let mut x: u64 = 42;
    let mut next = |m: u64| {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) % m
    };
    let mut ops = Vec::new();
    for _ in 0..10_000 {
        let (a, b) = (next(3000), next(3000));
        ops.push(CoreOp::Insert {
            s: iri(format!("n{a}")).into(),
            p: iri("knows".into()).into(),
            o: iri(format!("n{b}")).into(),
            graph: None,
        });
    }
    db.submit(WriteRequest { ops }, Durability::Relaxed)?;
    let t = std::time::Instant::now();
    for _ in 0..3 {
        let r = db.datalog("@prefix : <http://ex.org/> .\nreach(?x, ?y) :- triple(?x, :knows, ?y) .\nreach(?x, ?z) :- reach(?x, ?y), triple(?y, :knows, ?z) .\n?- reach(:n5, ?y) .")?;
        eprintln!("{} rows {:?}", r.rows().len(), t.elapsed());
    }
    Ok(())
}
