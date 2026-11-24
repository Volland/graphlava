//! Calls the C API through its `extern "C"` signatures with raw pointers, as a
//! C host would.

use std::ffi::{CStr, CString, c_char};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use graphlava_ffi::graphlava_status::*;
use graphlava_ffi::*;

fn temp_path(name: &str) -> CString {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let p = dir.join(format!(
        "ffi-{}-{}-{name}.glv",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_file(&p);
    CString::new(p.to_str().unwrap()).unwrap()
}

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn last_error() -> String {
    let mut p: *mut c_char = ptr::null_mut();
    // SAFETY: valid out-pointer.
    assert_eq!(unsafe { graphlava_last_error(&mut p) }, GRAPHLAVA_OK);
    assert!(!p.is_null(), "no last error recorded");
    // SAFETY: the library returned a NUL-terminated string.
    let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string();
    // SAFETY: returned by the library, freed once.
    unsafe { graphlava_string_free(p) };
    s
}

fn open(name: &str) -> *mut graphlava_db {
    let path = temp_path(name);
    let mut db = ptr::null_mut();
    // SAFETY: valid strings and out-pointer.
    let st = unsafe { graphlava_open(path.as_ptr(), &mut db) };
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());
    assert!(!db.is_null());
    db
}

const DATA: &str = r#"PREFIX : <http://ex.org/>
INSERT DATA {
  :alice :knows :bob .
  :bob :knows :carol .
  :alice :name "Alice"@en .
  :bob :age 42 .
}"#;

fn query(db: *mut graphlava_db, lang: graphlava_lang, text: &str) -> *mut graphlava_rows {
    let t = c(text);
    let mut prep = ptr::null_mut();
    // SAFETY: valid arguments.
    let st = unsafe { graphlava_prepare(db, lang, t.as_ptr(), &mut prep) };
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());
    let mut rows = ptr::null_mut();
    // SAFETY: valid handles and out-pointer.
    let st = unsafe { graphlava_query(db, prep, &mut rows) };
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());
    // SAFETY: the prepared handle is not used again; rows does not borrow it.
    unsafe { graphlava_prepared_free(prep) };
    rows
}

#[derive(Debug, PartialEq)]
enum T {
    Unbound,
    Iri(String),
    Blank,
    Lit(String, String, Option<String>),
    Triple(String),
}

fn decode(rows: *mut graphlava_rows, id: u64) -> T {
    let mut t = graphlava_term {
        kind: 99,
        value: ptr::null_mut(),
        datatype: ptr::null_mut(),
        lang: ptr::null_mut(),
    };
    // SAFETY: valid handle and out-pointer.
    let st = unsafe { graphlava_term_decode(rows, id, &mut t) };
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());
    let s = |p: *mut c_char| {
        // SAFETY: non-null strings from the library are NUL-terminated.
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string())
    };
    let out = match t.kind {
        GRAPHLAVA_TERM_UNBOUND => {
            assert!(t.value.is_null() && t.datatype.is_null() && t.lang.is_null());
            T::Unbound
        }
        GRAPHLAVA_TERM_IRI => T::Iri(s(t.value).unwrap()),
        GRAPHLAVA_TERM_BLANK => T::Blank,
        GRAPHLAVA_TERM_LITERAL => T::Lit(s(t.value).unwrap(), s(t.datatype).unwrap(), s(t.lang)),
        GRAPHLAVA_TERM_TRIPLE => {
            assert!(t.datatype.is_null() && t.lang.is_null());
            T::Triple(s(t.value).unwrap())
        }
        k => panic!("bad kind {k}"),
    };
    // SAFETY: filled by graphlava_term_decode.
    unsafe { graphlava_term_free(&mut t) };
    assert!(t.value.is_null());
    out
}

/// Reads all batches into row-major rows of terms.
fn collect(rows: *mut graphlava_rows) -> (Vec<String>, Vec<Vec<T>>) {
    let mut ncols = 0usize;
    // SAFETY: valid handle and out-pointer.
    assert_eq!(
        unsafe { graphlava_rows_columns(rows, &mut ncols) },
        GRAPHLAVA_OK
    );
    let mut names = Vec::new();
    for i in 0..ncols {
        let mut p = ptr::null_mut();
        // SAFETY: valid handle, index in range, out-pointer.
        assert_eq!(
            unsafe { graphlava_rows_column_name(rows, i, &mut p) },
            GRAPHLAVA_OK
        );
        // SAFETY: returned string.
        names.push(unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string());
        // SAFETY: freed once.
        unsafe { graphlava_string_free(p) };
    }
    let mut out = Vec::new();
    loop {
        let (mut ids, mut n, mut k) = (ptr::null(), 0usize, 0usize);
        // SAFETY: valid handle and out-pointers.
        let st = unsafe { graphlava_rows_next_batch(rows, &mut ids, &mut n, &mut k) };
        assert_eq!(st, GRAPHLAVA_OK);
        assert_eq!(k, ncols);
        if n == 0 {
            break;
        }
        assert!(n <= GRAPHLAVA_BATCH_ROWS);
        // SAFETY: the batch holds n * k IDs, valid until the next call.
        let batch: Vec<u64> = unsafe { std::slice::from_raw_parts(ids, n * k) }.to_vec();
        for r in 0..n {
            out.push((0..k).map(|col| decode(rows, batch[col * n + r])).collect());
        }
    }
    (names, out)
}

fn close(db: *mut graphlava_db) {
    // SAFETY: handle from graphlava_open with no live rows.
    let st = unsafe { graphlava_close(db) };
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());
}

fn iri(s: &str) -> T {
    T::Iri(format!("http://ex.org/{s}"))
}

#[test]
fn sparql_select_round_trip() {
    let db = open("select");
    let u = c(DATA);
    // SAFETY: valid arguments.
    let st = unsafe { graphlava_sparql_update(db, u.as_ptr(), GRAPHLAVA_DURABILITY_SYNC) };
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());

    let rows = query(
        db,
        GRAPHLAVA_LANG_SPARQL,
        "PREFIX : <http://ex.org/> SELECT ?x ?y ?n WHERE { ?x :knows ?y OPTIONAL { ?x :name ?n } } ORDER BY ?x",
    );
    let (names, mut got) = collect(rows);
    assert_eq!(names, ["x", "y", "n"]);
    got.sort_by_key(|r| format!("{r:?}"));
    assert_eq!(
        got,
        vec![
            vec![
                iri("alice"),
                iri("bob"),
                T::Lit(
                    "Alice".into(),
                    "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString".into(),
                    Some("en".into())
                )
            ],
            vec![iri("bob"), iri("carol"), T::Unbound],
        ]
    );
    // Exhausted handles keep returning empty batches.
    let (mut ids, mut n, mut k) = (ptr::null(), 7usize, 0usize);
    // SAFETY: valid handle and out-pointers.
    assert_eq!(
        unsafe { graphlava_rows_next_batch(rows, &mut ids, &mut n, &mut k) },
        GRAPHLAVA_OK
    );
    assert_eq!(n, 0);
    // SAFETY: freed once.
    unsafe { graphlava_rows_free(rows) };
    close(db);
}

#[test]
fn triple_term_decodes_as_ntriples() {
    let db = open("triple");
    let rows = query(
        db,
        GRAPHLAVA_LANG_SPARQL,
        "PREFIX : <http://ex.org/> SELECT ?t WHERE { BIND(TRIPLE(:a, :b, :c) AS ?t) }",
    );
    let (names, got) = collect(rows);
    assert_eq!(names, ["t"]);
    assert_eq!(
        got,
        vec![vec![T::Triple(
            "<<( <http://ex.org/a> <http://ex.org/b> <http://ex.org/c> )>>".into()
        )]]
    );
    // SAFETY: freed once.
    unsafe { graphlava_rows_free(rows) };
    close(db);
}

#[test]
fn datalog_goal_and_update() {
    let db = open("datalog");
    let u = c(DATA);
    // SAFETY: valid arguments.
    assert_eq!(
        unsafe { graphlava_sparql_update(db, u.as_ptr(), GRAPHLAVA_DURABILITY_ORDERED) },
        GRAPHLAVA_OK
    );
    let prog = c("@prefix : <http://ex.org/> .\n@assert :friend(X, Y) :- triple(X, :knows, Y) .\n");
    // SAFETY: valid arguments.
    let st = unsafe { graphlava_datalog_update(db, prog.as_ptr(), GRAPHLAVA_DURABILITY_RELAXED) };
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());

    let rows = query(
        db,
        GRAPHLAVA_LANG_DATALOG,
        "@prefix : <http://ex.org/> .\nreach(X, Y) :- triple(X, :knows, Y) .\nreach(X, Z) :- reach(X, Y), triple(Y, :knows, Z) .\n?- reach(:alice, Y) .\n",
    );
    let (names, mut got) = collect(rows);
    assert_eq!(names.len(), 1);
    got.sort_by_key(|r| format!("{r:?}"));
    assert_eq!(got, vec![vec![iri("bob")], vec![iri("carol")]]);
    // SAFETY: freed once.
    unsafe { graphlava_rows_free(rows) };

    // The `@assert` of the update was written.
    let rows = query(
        db,
        GRAPHLAVA_LANG_SPARQL,
        "PREFIX : <http://ex.org/> SELECT ?y WHERE { :alice :friend ?y }",
    );
    let (_, got) = collect(rows);
    assert_eq!(got, vec![vec![iri("bob")]]);
    // SAFETY: freed once.
    unsafe { graphlava_rows_free(rows) };
    close(db);
}

#[test]
fn core_insert() {
    let db = open("insert");
    let (s, p, o) = (
        c("http://ex.org/a"),
        c("http://ex.org/p"),
        c("http://ex.org/b"),
    );
    let mut id = u64::MAX;
    // SAFETY: valid arguments.
    let st = unsafe {
        graphlava_insert(
            db,
            s.as_ptr(),
            p.as_ptr(),
            GRAPHLAVA_TERM_IRI,
            o.as_ptr(),
            ptr::null(),
            ptr::null(),
            GRAPHLAVA_DURABILITY_SYNC,
            &mut id,
        )
    };
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());
    assert_ne!(id, u64::MAX);
    let (q, v, dt) = (
        c("http://ex.org/n"),
        c("7"),
        c("http://www.w3.org/2001/XMLSchema#integer"),
    );
    // SAFETY: valid arguments; out_id may be NULL.
    let st = unsafe {
        graphlava_insert(
            db,
            s.as_ptr(),
            q.as_ptr(),
            GRAPHLAVA_TERM_LITERAL,
            v.as_ptr(),
            dt.as_ptr(),
            ptr::null(),
            GRAPHLAVA_DURABILITY_ORDERED,
            ptr::null_mut(),
        )
    };
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());
    // Bad object kind.
    // SAFETY: valid pointers.
    let st = unsafe {
        graphlava_insert(
            db,
            s.as_ptr(),
            q.as_ptr(),
            42,
            v.as_ptr(),
            ptr::null(),
            ptr::null(),
            GRAPHLAVA_DURABILITY_SYNC,
            ptr::null_mut(),
        )
    };
    assert_eq!(st, GRAPHLAVA_ERR_INVALID_ARGUMENT);

    let rows = query(
        db,
        GRAPHLAVA_LANG_SPARQL,
        "SELECT ?o WHERE { <http://ex.org/a> ?p ?o }",
    );
    let (_, mut got) = collect(rows);
    got.sort_by_key(|r| format!("{r:?}"));
    assert_eq!(
        got,
        vec![
            vec![iri("b")],
            vec![T::Lit(
                "7".into(),
                "http://www.w3.org/2001/XMLSchema#integer".into(),
                None
            )],
        ]
    );
    // SAFETY: freed once.
    unsafe { graphlava_rows_free(rows) };
    close(db);
}

#[test]
fn batches_are_column_major_and_bounded() {
    let db = open("batches");
    let mut text = String::from("PREFIX : <http://ex.org/> INSERT DATA {");
    for i in 0..2500 {
        text.push_str(&format!(" :s{i} :v {i} ."));
    }
    text.push('}');
    let u = c(&text);
    // SAFETY: valid arguments.
    let st = unsafe { graphlava_sparql_update(db, u.as_ptr(), GRAPHLAVA_DURABILITY_RELAXED) };
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());
    let rows = query(
        db,
        GRAPHLAVA_LANG_SPARQL,
        "PREFIX : <http://ex.org/> SELECT ?s ?v WHERE { ?s :v ?v }",
    );
    let mut sizes = Vec::new();
    let mut seen = 0;
    loop {
        let (mut ids, mut n, mut k) = (ptr::null(), 0usize, 0usize);
        // SAFETY: valid handle and out-pointers.
        assert_eq!(
            unsafe { graphlava_rows_next_batch(rows, &mut ids, &mut n, &mut k) },
            GRAPHLAVA_OK
        );
        if n == 0 {
            break;
        }
        assert_eq!(k, 2);
        // SAFETY: n * k IDs.
        let b = unsafe { std::slice::from_raw_parts(ids, n * k) };
        // Check a few rows: column 0 is the subject, column 1 its value.
        for r in [0, n / 2, n - 1] {
            let T::Iri(s) = decode(rows, b[r]) else {
                panic!()
            };
            let T::Lit(v, _, _) = decode(rows, b[n + r]) else {
                panic!()
            };
            assert_eq!(s, format!("http://ex.org/s{v}"));
        }
        sizes.push(n);
        seen += n;
    }
    assert_eq!(seen, 2500);
    assert_eq!(sizes, [1024, 1024, 452]);
    // SAFETY: freed once.
    unsafe { graphlava_rows_free(rows) };
    close(db);
}

#[test]
fn close_refuses_while_rows_live() {
    let db = open("busy");
    let r1 = query(db, GRAPHLAVA_LANG_SPARQL, "SELECT ?s WHERE { ?s ?p ?o }");
    let r2 = query(db, GRAPHLAVA_LANG_SPARQL, "SELECT ?s WHERE { ?s ?p ?o }");
    // SAFETY: valid handle; refused, so it stays valid.
    assert_eq!(unsafe { graphlava_close(db) }, GRAPHLAVA_ERR_BUSY);
    assert!(last_error().contains("2 result handle"));
    // SAFETY: freed once.
    unsafe { graphlava_rows_free(r1) };
    // SAFETY: as above.
    assert_eq!(unsafe { graphlava_close(db) }, GRAPHLAVA_ERR_BUSY);
    // The database is still usable after a refused close.
    let u = c(DATA);
    // SAFETY: valid arguments.
    assert_eq!(
        unsafe { graphlava_sparql_update(db, u.as_ptr(), GRAPHLAVA_DURABILITY_SYNC) },
        GRAPHLAVA_OK
    );
    // SAFETY: freed once.
    unsafe { graphlava_rows_free(r2) };
    close(db);
}

#[test]
fn second_open_is_locked() {
    let path = temp_path("locked");
    let (mut a, mut b) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: valid arguments.
    assert_eq!(
        unsafe { graphlava_open(path.as_ptr(), &mut a) },
        GRAPHLAVA_OK
    );
    // SAFETY: valid arguments.
    assert_eq!(
        unsafe { graphlava_open(path.as_ptr(), &mut b) },
        GRAPHLAVA_ERR_LOCKED
    );
    assert!(b.is_null());
    close(a);
}

#[test]
fn invalid_query_reports_status_and_message() {
    let db = open("invalid");
    let rows = {
        let t = c("SELEKT nothing");
        let mut prep = ptr::null_mut();
        // SAFETY: valid arguments.
        assert_eq!(
            unsafe { graphlava_prepare(db, GRAPHLAVA_LANG_SPARQL, t.as_ptr(), &mut prep) },
            GRAPHLAVA_OK
        );
        let mut rows = ptr::dangling_mut::<graphlava_rows>();
        // SAFETY: valid arguments.
        let st = unsafe { graphlava_query(db, prep, &mut rows) };
        assert_eq!(st, GRAPHLAVA_ERR_INVALID_REQUEST);
        // SAFETY: freed once.
        unsafe { graphlava_prepared_free(prep) };
        rows
    };
    assert!(rows.is_null());
    let msg = last_error();
    assert!(!msg.is_empty());
    // A failed query does not leave a live result behind.
    let bad = c("INSERT DATA { broken");
    // SAFETY: valid arguments.
    assert_eq!(
        unsafe { graphlava_sparql_update(db, bad.as_ptr(), GRAPHLAVA_DURABILITY_SYNC) },
        GRAPHLAVA_ERR_INVALID_REQUEST
    );
    let bad = c("this is not datalog");
    // SAFETY: valid arguments.
    assert_eq!(
        unsafe { graphlava_datalog_update(db, bad.as_ptr(), GRAPHLAVA_DURABILITY_SYNC) },
        GRAPHLAVA_ERR_INVALID_REQUEST
    );
    close(db);
}

#[test]
fn null_and_bad_utf8_arguments() {
    let db = open("args");
    let mut out_db = ptr::null_mut();
    // SAFETY (all calls below): the library must reject the bad pointers
    // without dereferencing them.
    unsafe {
        assert_eq!(
            graphlava_open(ptr::null(), &mut out_db),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert!(last_error().contains("path"));
        let p = c("x.glv");
        assert_eq!(
            graphlava_open(p.as_ptr(), ptr::null_mut()),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            graphlava_close(ptr::null_mut()),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            graphlava_last_error(ptr::null_mut()),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );

        let bad: &[u8] = b"SELECT \xff\xfe\0";
        let bad = bad.as_ptr().cast::<c_char>();
        assert_eq!(
            graphlava_sparql_update(db, bad, GRAPHLAVA_DURABILITY_SYNC),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert!(last_error().contains("UTF-8"));
        let mut prep = ptr::null_mut();
        assert_eq!(
            graphlava_prepare(db, GRAPHLAVA_LANG_SPARQL, bad, &mut prep),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            graphlava_prepare(db, 9, c("SELECT * {}").as_ptr(), &mut prep),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert!(prep.is_null());
        let u = c(DATA);
        assert_eq!(
            graphlava_sparql_update(db, u.as_ptr(), 77),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            graphlava_sparql_update(ptr::null(), u.as_ptr(), GRAPHLAVA_DURABILITY_SYNC),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            graphlava_datalog_update(db, ptr::null(), GRAPHLAVA_DURABILITY_SYNC),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            graphlava_insert(
                db,
                ptr::null(),
                bad,
                GRAPHLAVA_TERM_IRI,
                bad,
                ptr::null(),
                ptr::null(),
                GRAPHLAVA_DURABILITY_SYNC,
                ptr::null_mut()
            ),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        let mut rows = ptr::null_mut();
        assert_eq!(
            graphlava_query(db, ptr::null(), &mut rows),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        let mut n = 0usize;
        assert_eq!(
            graphlava_rows_columns(ptr::null(), &mut n),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        let (mut ids, mut k) = (ptr::null(), 0usize);
        assert_eq!(
            graphlava_rows_next_batch(ptr::null_mut(), &mut ids, &mut n, &mut k),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        let mut t = std::mem::zeroed::<graphlava_term>();
        assert_eq!(
            graphlava_term_decode(ptr::null(), 1, &mut t),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );

        // Out-of-range column and null out-pointers on a live result.
        let r = query(db, GRAPHLAVA_LANG_SPARQL, "SELECT ?s WHERE { ?s ?p ?o }");
        let mut name = ptr::null_mut();
        assert_eq!(
            graphlava_rows_column_name(r, 5, &mut name),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert!(name.is_null());
        assert_eq!(
            graphlava_rows_columns(r, ptr::null_mut()),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        assert_eq!(
            graphlava_term_decode(r, 1, ptr::null_mut()),
            GRAPHLAVA_ERR_INVALID_ARGUMENT
        );
        graphlava_rows_free(r);

        // The free functions accept NULL.
        graphlava_rows_free(ptr::null_mut());
        graphlava_prepared_free(ptr::null_mut());
        graphlava_string_free(ptr::null_mut());
        graphlava_term_free(ptr::null_mut());
    }
    close(db);
}

#[test]
fn last_error_is_per_thread() {
    // SAFETY: rejected without dereferencing.
    assert_eq!(
        unsafe { graphlava_close(ptr::null_mut()) },
        GRAPHLAVA_ERR_INVALID_ARGUMENT
    );
    std::thread::spawn(|| {
        let mut p: *mut c_char = ptr::dangling_mut::<c_char>();
        // SAFETY: valid out-pointer.
        assert_eq!(unsafe { graphlava_last_error(&mut p) }, GRAPHLAVA_OK);
        assert!(p.is_null());
    })
    .join()
    .unwrap();
    assert!(last_error().contains("db"));
}
