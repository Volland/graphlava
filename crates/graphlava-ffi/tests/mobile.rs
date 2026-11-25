//! Suspend, resume and the iOS extension journal through the C API
//! ([[16-api#Mobile bindings]], change `v1-mobile-bindings`). The Swift and
//! Kotlin packages call exactly these functions; their own suites repeat the
//! scenarios through the bindings.

use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use graphlava::{Database, Error, OpenOptions};
use graphlava_ffi::graphlava_status::*;
use graphlava_ffi::*;

fn temp(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "mobile-{}-{}-{name}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_file(&p);
    p
}

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn cp(p: &Path) -> CString {
    c(p.to_str().unwrap())
}

fn last_error() -> String {
    let mut p: *mut c_char = ptr::null_mut();
    // SAFETY: valid out-pointer; the string is freed once.
    unsafe {
        graphlava_last_error(&mut p);
        if p.is_null() {
            return String::new();
        }
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        graphlava_string_free(p);
        s
    }
}

fn ok(st: graphlava_status) {
    assert_eq!(st, GRAPHLAVA_OK, "{}", last_error());
}

fn open(path: &Path, flags: u32) -> *mut graphlava_db {
    let mut db = ptr::null_mut();
    // SAFETY: valid strings and out-pointer.
    ok(unsafe { graphlava_open_ex(GRAPHLAVA_ABI_VERSION, cp(path).as_ptr(), flags, &mut db) });
    db
}

fn close(db: *mut graphlava_db) {
    // SAFETY: a live handle.
    ok(unsafe { graphlava_close(db) });
}

fn update(db: *mut graphlava_db, text: &str) {
    // SAFETY: valid arguments.
    ok(unsafe { graphlava_sparql_update(db, c(text).as_ptr(), GRAPHLAVA_DURABILITY_ORDERED) });
}

fn text_request(text: &str) -> *mut graphlava_request {
    let mut r = ptr::null_mut();
    // SAFETY: valid arguments.
    ok(unsafe { graphlava_request_new_text(GRAPHLAVA_LANG_SPARQL, c(text).as_ptr(), &mut r) });
    r
}

fn rows_of(db: *mut graphlava_db, q: &str) -> usize {
    let mut p = ptr::null_mut();
    let mut rows = ptr::null_mut();
    // SAFETY: valid arguments; handles are freed once.
    unsafe {
        ok(graphlava_prepare(
            db,
            GRAPHLAVA_LANG_SPARQL,
            c(q).as_ptr(),
            &mut p,
        ));
        ok(graphlava_query(db, p, &mut rows));
        graphlava_prepared_free(p);
        let mut n = 0;
        loop {
            let (mut ids, mut nr, mut nc) = (ptr::null(), 0, 0);
            ok(graphlava_rows_next_batch(rows, &mut ids, &mut nr, &mut nc));
            if nr == 0 {
                break;
            }
            n += nr;
        }
        graphlava_rows_free(rows);
        n
    }
}

const P: &str = "PREFIX : <http://ex.org/>\n";

// ---- suspend and resume ------------------------------------------------------------

#[test]
fn suspend_ends_snapshots_and_aborts_the_write() {
    let path = temp("susp.glv");
    let db = open(&path, GRAPHLAVA_OPEN_APP_GROUP);
    update(db, &format!("{P}INSERT DATA {{ :a :p :b . :a :p :c }}"));
    // SAFETY: valid handles throughout; each is ended once.
    unsafe {
        let mut prep = ptr::null_mut();
        let mut rows = ptr::null_mut();
        ok(graphlava_prepare(
            db,
            GRAPHLAVA_LANG_SPARQL,
            c("SELECT * { ?s ?p ?o }").as_ptr(),
            &mut prep,
        ));
        ok(graphlava_query(db, prep, &mut rows));
        let mut w = ptr::null_mut();
        ok(graphlava_write_begin(db, GRAPHLAVA_DURABILITY_SYNC, &mut w));
        let req = text_request(&format!("{P}INSERT DATA {{ :x :p :y }}"));
        ok(graphlava_write_run(w, req, ptr::null_mut()));

        ok(graphlava_suspend(db));
        let (mut ids, mut nr, mut nc) = (ptr::null(), 0, 0);
        assert_eq!(
            graphlava_rows_next_batch(rows, &mut ids, &mut nr, &mut nc),
            GRAPHLAVA_ERR_CLOSED
        );
        let mut t = std::mem::zeroed::<graphlava_term>();
        assert_eq!(graphlava_term_decode(rows, 1, &mut t), GRAPHLAVA_ERR_CLOSED);
        assert_eq!(
            graphlava_write_run(w, req, ptr::null_mut()),
            GRAPHLAVA_ERR_ABORTED
        );
        assert_eq!(graphlava_commit(w), GRAPHLAVA_ERR_ABORTED);
        assert_eq!(
            graphlava_commit(w),
            GRAPHLAVA_ERR_CLOSED,
            "commit freed the handle"
        );
        // The app-group file is released: another owner can open it.
        let other = Database::open(&path, OpenOptions::default()).expect("lock released");
        other.close().unwrap();
        ok(graphlava_resume(db));
        graphlava_rows_free(rows);
        graphlava_prepared_free(prep);
        graphlava_request_free(req);
    }
    assert_eq!(
        rows_of(db, &format!("{P}SELECT * {{ :x :p :y }}")),
        0,
        "aborted"
    );
    assert_eq!(rows_of(db, &format!("{P}SELECT * {{ :a :p ?o }}")), 2);
    let mut n = 9;
    // SAFETY: valid arguments.
    ok(unsafe { graphlava_open_snapshots(db, &mut n) });
    assert_eq!(n, 0);
    close(db);
}

// @tg: verifies:: [[openspec:mobile-bindings#iOS suspend and resume#Calls while suspended wait for resume]] {level: e2e}
// @tg: verifies:: [[21-e2e-tests#iOS suspend during a group commit]] {level: e2e}
#[test]
fn calls_while_suspended_wait_for_resume() {
    let path = temp("wait.glv");
    let db = open(&path, GRAPHLAVA_OPEN_APP_GROUP);
    update(db, &format!("{P}INSERT DATA {{ :a :p :b }}"));
    // SAFETY: valid handle.
    ok(unsafe { graphlava_suspend(db) });
    let tok = db as usize;
    let started = Instant::now();
    let reader = std::thread::spawn(move || {
        let n = rows_of(
            tok as *mut graphlava_db,
            &format!("{P}SELECT * {{ :a :p ?o }}"),
        );
        (n, started.elapsed())
    });
    std::thread::sleep(Duration::from_millis(300));
    // SAFETY: valid handle.
    ok(unsafe { graphlava_resume(db) });
    let (n, waited) = reader.join().unwrap();
    assert_eq!(n, 1, "correct rows after resume");
    assert!(
        waited >= Duration::from_millis(300),
        "the read waited: {waited:?}"
    );
    close(db);
}

// @tg: verifies:: [[openspec:mobile-bindings#iOS suspend and resume#Private container keeps the lock]] {level: e2e}
#[test]
fn private_container_keeps_the_lock() {
    let path = temp("private.glv");
    let db = open(&path, 0);
    // SAFETY: valid arguments.
    ok(unsafe {
        graphlava_sparql_update(
            db,
            c(&format!("{P}INSERT DATA {{ :a :p :b }}")).as_ptr(),
            GRAPHLAVA_DURABILITY_RELAXED,
        )
    });
    // SAFETY: valid handle.
    ok(unsafe { graphlava_suspend(db) });
    assert!(matches!(
        Database::open(&path, OpenOptions::default()),
        Err(Error::Locked)
    ));
    // SAFETY: valid handle.
    ok(unsafe { graphlava_resume(db) });
    assert_eq!(rows_of(db, &format!("{P}SELECT * {{ :a :p :b }}")), 1);
    close(db);
}

// @tg: verifies:: [[openspec:mobile-bindings#iOS suspend and resume#Suspend with nothing open]] {level: unit}
#[test]
fn suspend_with_nothing_open() {
    let path = temp("idle.glv");
    let db = open(&path, 0);
    let t = Instant::now();
    // SAFETY: valid handle.
    ok(unsafe { graphlava_suspend(db) });
    assert!(t.elapsed() < Duration::from_millis(5), "{:?}", t.elapsed());
    // SAFETY: valid handle.
    ok(unsafe { graphlava_suspend(db) });
    let mut s = 0;
    // SAFETY: valid arguments.
    ok(unsafe { graphlava_is_suspended(db, &mut s) });
    assert_eq!(s, 1);
    // SAFETY: valid handle.
    ok(unsafe { graphlava_resume(db) });
    close(db);
}

// @tg: verifies:: [[openspec:mobile-bindings#Complete C FFI#Header version mismatch]] {level: unit}
#[test]
fn header_version_mismatch() {
    let path = temp("abi.glv");
    let mut db = ptr::null_mut();
    // SAFETY: valid arguments.
    let st =
        unsafe { graphlava_open_ex(GRAPHLAVA_ABI_VERSION + 1, cp(&path).as_ptr(), 0, &mut db) };
    assert_eq!(st, GRAPHLAVA_ERR_ABI);
    assert!(db.is_null());
    assert!(!path.exists(), "no file is created");
}

// ---- extension journal ---------------------------------------------------------------

fn append(journal: &Path, text: &str) -> u64 {
    let req = text_request(text);
    let mut seq = 0;
    // SAFETY: valid arguments; the request is freed once.
    unsafe {
        ok(graphlava_journal_append(
            cp(journal).as_ptr(),
            req,
            &mut seq,
        ));
        graphlava_request_free(req);
    }
    seq
}

fn apply(db: *mut graphlava_db, journal: &Path) -> graphlava_journal_report {
    let mut rep = graphlava_journal_report::default();
    // SAFETY: valid arguments.
    ok(unsafe { graphlava_journal_apply(db, cp(journal).as_ptr(), c("share").as_ptr(), &mut rep) });
    rep
}

/// Three requests whose final state reveals their order: `:n :v 3` only when
/// they ran 1, 2, 3.
fn chain() -> [String; 3] {
    [
        format!("{P}INSERT DATA {{ :n :v 1 }}"),
        format!("{P}DELETE {{ :n :v 1 }} INSERT {{ :n :v 2 }} WHERE {{ :n :v 1 }}"),
        format!("{P}DELETE {{ :n :v 2 }} INSERT {{ :n :v 3 }} WHERE {{ :n :v 2 }}"),
    ]
}

// @tg: verifies:: [[openspec:mobile-bindings#iOS extension writes#Share extension write while app is closed]] {level: e2e}
#[test]
fn share_extension_write_while_app_is_closed() {
    let path = temp("share.glv");
    let journal = temp("share.journal");
    // The app ran once, then closed.
    close(open(&path, GRAPHLAVA_OPEN_APP_GROUP));
    // The extension submits three requests without opening the database.
    for (i, t) in chain().iter().enumerate() {
        assert_eq!(append(&journal, t), i as u64 + 1);
    }
    // The app launches and applies them in order.
    let db = open(&path, GRAPHLAVA_OPEN_APP_GROUP);
    let rep = apply(db, &journal);
    assert_eq!(
        (rep.applied, rep.skipped, rep.failed, rep.dropped_bytes),
        (3, 0, 0, 0)
    );
    assert_eq!(rows_of(db, &format!("{P}SELECT * {{ :n :v 3 }}")), 1);
    assert_eq!(rows_of(db, &format!("{P}SELECT * {{ :n :v ?x }}")), 1);
    assert_eq!(
        std::fs::metadata(&journal).unwrap().len(),
        16,
        "journal is empty"
    );
    close(db);
    // Relaunching does not apply them again; numbering continues.
    let db = open(&path, GRAPHLAVA_OPEN_APP_GROUP);
    assert_eq!(apply(db, &journal).applied, 0);
    assert_eq!(
        append(&journal, &format!("{P}INSERT DATA {{ :m :v 1 }}")),
        4
    );
    assert_eq!(apply(db, &journal).applied, 1);
    close(db);
}

// @tg: verifies:: [[openspec:mobile-bindings#iOS extension writes#Corrupt journal entry]] {level: e2e}
#[test]
fn corrupt_journal_entry() {
    let path = temp("torn.glv");
    let journal = temp("torn.journal");
    for t in chain() {
        append(&journal, &t);
    }
    // The extension was killed mid-append: the last frame is torn.
    let mut b = std::fs::read(&journal).unwrap();
    let n = b.len();
    b[n - 1] ^= 0xFF;
    std::fs::write(&journal, &b).unwrap();
    let db = open(&path, 0);
    let rep = apply(db, &journal);
    assert_eq!(rep.applied, 2);
    assert!(
        rep.dropped_bytes > 0,
        "one warning's worth of dropped bytes"
    );
    assert_eq!(rows_of(db, &format!("{P}SELECT * {{ :n :v 2 }}")), 1);
    assert_eq!(std::fs::metadata(&journal).unwrap().len(), 16);
    close(db);
}

/// The child half of the crash test: applies the journal and aborts the
/// process right after the second frame commits.
#[test]
fn journal_crash_child() {
    let (Ok(db_path), Ok(journal)) = (
        std::env::var("GRAPHLAVA_CRASH_DB"),
        std::env::var("GRAPHLAVA_CRASH_JOURNAL"),
    ) else {
        return; // only runs as the child of `crash_while_applying_the_journal`
    };
    let db = open(Path::new(&db_path), GRAPHLAVA_OPEN_APP_GROUP);
    let _ = journal_apply_hooked(db, Path::new(&journal), "share", |seq| {
        if seq == 2 {
            std::process::abort();
        }
    });
    unreachable!("the hook aborts after frame 2");
}

// @tg: verifies:: [[openspec:mobile-bindings#iOS extension writes#Crash while applying the journal]] {level: crash}
// @tg: verifies:: [[21-e2e-tests#Extension journal applied once]] {level: crash}
#[test]
fn crash_while_applying_the_journal() {
    let path = temp("crash.glv");
    let journal = temp("crash.journal");
    // A multi predicate: re-applying a frame would add a second statement.
    let db = open(&path, 0);
    // SAFETY: valid arguments.
    unsafe {
        let mut req = ptr::null_mut();
        ok(graphlava_request_new(&mut req));
        let p = c("http://ex.org/seen");
        let args = [
            graphlava_arg {
                kind: GRAPHLAVA_TERM_IRI,
                id: 0,
                value: p.as_ptr(),
                datatype: ptr::null(),
                lang: ptr::null(),
            },
            graphlava_arg {
                kind: GRAPHLAVA_ARG_UINT,
                id: 1,
                value: ptr::null(),
                datatype: ptr::null(),
                lang: ptr::null(),
            },
        ];
        ok(graphlava_request_add(
            req,
            GRAPHLAVA_OP_DECLARE_PREDICATE,
            args.as_ptr(),
            2,
        ));
        ok(graphlava_submit(
            db,
            req,
            GRAPHLAVA_DURABILITY_SYNC,
            ptr::null_mut(),
        ));
        graphlava_request_free(req);
    }
    close(db);
    for i in 1..=3 {
        append(&journal, &format!("{P}INSERT DATA {{ :e{i} :seen :x }}"));
    }
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "journal_crash_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("GRAPHLAVA_CRASH_DB", &path)
        .env("GRAPHLAVA_CRASH_JOURNAL", &journal)
        .output()
        .unwrap();
    assert!(!child.status.success(), "the child was killed");
    let db = open(&path, 0);
    assert_eq!(
        rows_of(db, &format!("{P}SELECT * {{ ?e :seen :x }}")),
        2,
        "two applied before the kill"
    );
    let rep = apply(db, &journal);
    assert_eq!(
        (rep.applied, rep.skipped),
        (1, 2),
        "exactly the third entry is applied"
    );
    assert_eq!(rows_of(db, &format!("{P}SELECT * {{ ?e :seen :x }}")), 3);
    assert_eq!(
        rows_of(db, &format!("{P}SELECT * {{ :e1 :seen :x }}")),
        1,
        "not applied again"
    );
    close(db);
}

#[test]
fn request_bytes_round_trip_through_the_c_api() {
    let req = text_request(&format!("{P}INSERT DATA {{ :a :p :b }}"));
    // SAFETY: valid arguments; buffers sized from the first call.
    unsafe {
        let mut len = 0;
        ok(graphlava_request_encode(req, ptr::null_mut(), 0, &mut len));
        let mut buf = vec![0u8; len];
        let mut len2 = 0;
        ok(graphlava_request_encode(
            req,
            buf.as_mut_ptr(),
            buf.len(),
            &mut len2,
        ));
        assert_eq!(len, len2);
        let mut back = ptr::null_mut();
        ok(graphlava_request_decode(buf.as_ptr(), buf.len(), &mut back));
        let path = temp("rt.glv");
        let db = open(&path, 0);
        ok(graphlava_submit(
            db,
            back,
            GRAPHLAVA_DURABILITY_SYNC,
            ptr::null_mut(),
        ));
        assert_eq!(rows_of(db, &format!("{P}SELECT * {{ :a :p :b }}")), 1);
        close(db);
        graphlava_request_free(back);
        graphlava_request_free(req);
    }
}
