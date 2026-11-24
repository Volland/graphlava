//! Random sequences of C FFI calls, including use after close, double commit,
//! wrong handle kinds, stale handles and null out-parameters
//! ([[openspec:mobile-bindings#FFI misuse safety]]). Every call must return a
//! status from the documented set, and no call may panic (a caught panic is
//! `GRAPHLAVA_ERR_INTERNAL`, which fails the run). Run under the fuzzer's
//! AddressSanitizer build, memory errors and leaks fail the run too.
#![no_main]

use std::ffi::{CString, c_char};
use std::ptr;
use std::sync::OnceLock;

use graphlava_ffi::graphlava_status::*;
use graphlava_ffi::*;
use libfuzzer_sys::fuzz_target;

fn dir() -> &'static std::path::Path {
    static D: OnceLock<std::path::PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("graphlava-fuzz-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    })
}

const TEXTS: &[&str] = &[
    "PREFIX : <http://ex.org/> INSERT DATA { :a :p :b . :a :q 42 }",
    "PREFIX : <http://ex.org/> SELECT * WHERE { ?s ?p ?o }",
    "PREFIX : <http://ex.org/> DELETE WHERE { :a ?p ?o }",
    "SELEKT nonsense",
    "?- <http://ex.org/p>(?x, ?y).",
    "",
];

const IRIS: &[&str] = &["http://ex.org/a", "http://ex.org/p", "http://ex.org/b", "x"];

struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn u8(&mut self) -> Option<u8> {
        let (b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(*b)
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> Option<T> {
        if xs.is_empty() {
            return None;
        }
        Some(xs[self.u8()? as usize % xs.len()])
    }
}

fn check(st: graphlava_status) {
    assert_ne!(st, GRAPHLAVA_ERR_INTERNAL, "a call panicked or failed internally");
}

#[derive(Default)]
struct State {
    dbs: Vec<(*mut graphlava_db, bool)>, // (handle, suspended)
    reads: Vec<*mut graphlava_read>,
    writes: Vec<*mut graphlava_write>,
    reqs: Vec<*mut graphlava_request>,
    rows: Vec<*mut graphlava_rows>,
    preps: Vec<*mut graphlava_prepared>,
    results: Vec<*mut graphlava_result>,
}

impl State {
    fn db(&mut self, b: &mut Bytes) -> Option<usize> {
        if self.dbs.is_empty() {
            return None;
        }
        Some(b.u8()? as usize % self.dbs.len())
    }
    /// Calls that wait while suspended would hang this single-threaded
    /// harness, so resume first (waiting is the documented behaviour).
    fn awake(&mut self, i: usize) {
        if self.dbs[i].1 {
            // SAFETY: a handle or stale token; never dereferenced by the library.
            check(unsafe { graphlava_resume(self.dbs[i].0) });
            self.dbs[i].1 = false;
        }
    }
}

fn arg(b: &mut Bytes, keep: &mut Vec<CString>) -> Option<graphlava_arg> {
    let kind = b.pick(&[0u32, 1, 2, 3, 5, 6, 9])?;
    let s = CString::new(b.pick(IRIS)?).unwrap();
    let p = if b.u8()? % 8 == 0 { ptr::null() } else { s.as_ptr() };
    keep.push(s);
    Some(graphlava_arg {
        kind,
        id: u64::from(b.u8()?) | (u64::from(b.u8()? % 16) << 60),
        value: p,
        datatype: ptr::null(),
        lang: ptr::null(),
    })
}

// @tg: verifies:: [[openspec:mobile-bindings#FFI misuse safety#Random call sequences]] {level: property}
fuzz_target!(|data: &[u8]| {
    let mut b = Bytes(data);
    let mut s = State::default();
    let path = CString::new(dir().join("f.glv").to_str().unwrap()).unwrap();
    let journal = CString::new(dir().join("f.journal").to_str().unwrap()).unwrap();
    let mut keep: Vec<CString> = Vec::new();
    // SAFETY: every pointer passed is a token from this library (possibly
    // stale), NULL, or a valid local buffer; the library never dereferences
    // tokens.
    unsafe {
        while let Some(op) = b.u8() {
            match op % 24 {
                0 => {
                    let mut db = ptr::null_mut();
                    let abi = if b.u8().unwrap_or(0) % 16 == 0 { 7 } else { GRAPHLAVA_ABI_VERSION };
                    let st = graphlava_open_ex(abi, path.as_ptr(), u32::from(b.u8().unwrap_or(0) % 3), &mut db);
                    check(st);
                    if st == GRAPHLAVA_OK {
                        s.dbs.push((db, false));
                    }
                }
                1 => {
                    if let Some(i) = s.db(&mut b) {
                        check(graphlava_close(s.dbs[i].0)); // may be BUSY or CLOSED
                    }
                }
                2 => {
                    if let Some(i) = s.db(&mut b) {
                        let st = graphlava_suspend(s.dbs[i].0);
                        check(st);
                        // BUSY also leaves the database suspended.
                        s.dbs[i].1 |= st == GRAPHLAVA_OK || st == GRAPHLAVA_ERR_BUSY;
                    }
                }
                3 => {
                    if let Some(i) = s.db(&mut b) {
                        s.awake(i);
                        check(graphlava_flush(s.dbs[i].0));
                    }
                }
                4 => {
                    if let Some(i) = s.db(&mut b) {
                        s.awake(i);
                        let mut r = ptr::null_mut();
                        let out = if b.u8().unwrap_or(1) == 0 { ptr::null_mut() } else { &mut r };
                        check(graphlava_read_begin(s.dbs[i].0, out));
                        if !r.is_null() {
                            s.reads.push(r);
                        }
                    }
                }
                5 => {
                    if let Some(r) = b.pick(&s.reads.clone()) {
                        graphlava_read_end(r); // double end is ignored
                    }
                }
                6 => {
                    if let Some(r) = b.pick(&s.reads.clone()) {
                        let mut buf = [0u64; 16];
                        let mut n = 0;
                        let cap = usize::from(b.u8().unwrap_or(0) % 5);
                        check(graphlava_find(r, 0, 0, 0, buf.as_mut_ptr(), cap, &mut n));
                        let a = arg(&mut b, &mut keep).unwrap_or(graphlava_arg {
                            kind: 1,
                            id: 0,
                            value: ptr::null(),
                            datatype: ptr::null(),
                            lang: ptr::null(),
                        });
                        let mut id = 0;
                        check(graphlava_lookup(r, &a, &mut id));
                        let mut t = std::mem::zeroed::<graphlava_term>();
                        check(graphlava_decode(r, u64::from(b.u8().unwrap_or(0)), &mut t));
                        graphlava_term_free(&mut t);
                        let mut spo = [0u64; 3];
                        check(graphlava_stmt(r, id, spo.as_mut_ptr()));
                    }
                }
                7 => {
                    if let Some(i) = s.db(&mut b) {
                        s.awake(i);
                        let mut w = ptr::null_mut();
                        check(graphlava_write_begin(s.dbs[i].0, u32::from(b.u8().unwrap_or(0) % 4), &mut w));
                        if !w.is_null() {
                            s.writes.push(w);
                        }
                    }
                }
                8 => {
                    if let (Some(w), Some(r)) = (b.pick(&s.writes.clone()), b.pick(&s.reqs.clone())) {
                        let mut res = ptr::null_mut();
                        check(graphlava_write_run(w, r, &mut res));
                        if !res.is_null() {
                            s.results.push(res);
                        }
                    }
                }
                9 => {
                    if let Some(w) = b.pick(&s.writes.clone()) {
                        check(graphlava_commit(w)); // double commit gives CLOSED
                    }
                }
                10 => {
                    if let Some(w) = b.pick(&s.writes.clone()) {
                        check(graphlava_abort(w));
                    }
                }
                11 => {
                    let mut r = ptr::null_mut();
                    check(graphlava_request_new(&mut r));
                    s.reqs.push(r);
                }
                12 => {
                    let t = CString::new(b.pick(TEXTS).unwrap_or("")).unwrap();
                    let mut r = ptr::null_mut();
                    check(graphlava_request_new_text(u32::from(b.u8().unwrap_or(0) % 3), t.as_ptr(), &mut r));
                    if !r.is_null() {
                        s.reqs.push(r);
                    }
                }
                13 => {
                    if let Some(r) = b.pick(&s.reqs.clone()) {
                        let n = usize::from(b.u8().unwrap_or(0) % 6);
                        let mut args = Vec::new();
                        for _ in 0..n {
                            if let Some(a) = arg(&mut b, &mut keep) {
                                args.push(a);
                            }
                        }
                        let code = u32::from(b.u8().unwrap_or(0) % 24);
                        check(graphlava_request_add(r, code, args.as_ptr(), args.len()));
                    }
                }
                14 => {
                    if let (Some(i), Some(r)) = (s.db(&mut b), b.pick(&s.reqs.clone())) {
                        s.awake(i);
                        let mut res = ptr::null_mut();
                        check(graphlava_submit(s.dbs[i].0, r, u32::from(b.u8().unwrap_or(0) % 4), &mut res));
                        if !res.is_null() {
                            s.results.push(res);
                        }
                    }
                }
                15 => {
                    if let Some(r) = b.pick(&s.reqs.clone()) {
                        graphlava_request_free(r);
                    }
                }
                16 => {
                    let t = CString::new(b.pick(TEXTS).unwrap_or("")).unwrap();
                    let mut p = ptr::null_mut();
                    check(graphlava_prepare(ptr::null(), u32::from(b.u8().unwrap_or(0) % 3), t.as_ptr(), &mut p));
                    if !p.is_null() {
                        s.preps.push(p);
                    }
                }
                17 => {
                    if let (Some(i), Some(p)) = (s.db(&mut b), b.pick(&s.preps.clone())) {
                        s.awake(i);
                        let mut r = ptr::null_mut();
                        check(graphlava_query(s.dbs[i].0, p, &mut r));
                        if !r.is_null() {
                            s.rows.push(r);
                        }
                    }
                }
                18 => {
                    if let Some(r) = b.pick(&s.rows.clone()) {
                        let (mut ids, mut nr, mut nc) = (ptr::null(), 0, 0);
                        let st = graphlava_rows_next_batch(r, &mut ids, &mut nr, &mut nc);
                        check(st);
                        if st == GRAPHLAVA_OK && nr > 0 {
                            let id = *ids;
                            let mut t = std::mem::zeroed::<graphlava_term>();
                            check(graphlava_term_decode(r, id, &mut t));
                            graphlava_term_free(&mut t);
                        }
                    }
                }
                19 => {
                    if let Some(r) = b.pick(&s.rows.clone()) {
                        graphlava_rows_free(r);
                    }
                }
                20 => {
                    if let Some(r) = b.pick(&s.results.clone()) {
                        let (mut k, mut v) = (0, 0);
                        check(graphlava_result_get(r, usize::from(b.u8().unwrap_or(0) % 3), &mut k, &mut v));
                        let mut n = 0;
                        check(graphlava_result_deleted(r, 0, ptr::null_mut(), 0, &mut n));
                        if b.u8().unwrap_or(0) % 2 == 0 {
                            graphlava_result_free(r);
                        }
                    }
                }
                21 => {
                    // Wrong handle kinds and garbage pointers.
                    let junk = (u64::from(b.u8().unwrap_or(0)) * 8) as usize;
                    check(graphlava_flush(junk as *mut graphlava_db));
                    if let Some(r) = b.pick(&s.reads.clone()) {
                        check(graphlava_flush(r.cast()));
                    }
                    let mut n = 0;
                    check(graphlava_rows_columns(junk as *const graphlava_rows, &mut n));
                }
                22 => {
                    // Null out-parameters.
                    if let Some(i) = s.db(&mut b) {
                        check(graphlava_is_suspended(s.dbs[i].0, ptr::null_mut()));
                        check(graphlava_open_snapshots(s.dbs[i].0, ptr::null_mut()));
                    }
                    check(graphlava_request_new(ptr::null_mut()));
                    check(graphlava_abi_version(ptr::null_mut()));
                    let mut msg: *mut c_char = ptr::null_mut();
                    check(graphlava_last_error(&mut msg));
                    graphlava_string_free(msg);
                }
                _ => {
                    if let (Some(i), Some(r)) = (s.db(&mut b), b.pick(&s.reqs.clone())) {
                        s.awake(i);
                        check(graphlava_journal_append(journal.as_ptr(), r, ptr::null_mut()));
                        let mut rep = graphlava_journal_report::default();
                        check(graphlava_journal_apply(s.dbs[i].0, journal.as_ptr(), c"fz".as_ptr(), &mut rep));
                    }
                }
            }
        }
        // End everything so nothing leaks into the next input.
        for r in s.rows {
            graphlava_rows_free(r);
        }
        for r in s.reads {
            graphlava_read_end(r);
        }
        for w in s.writes {
            check(graphlava_abort(w));
        }
        for r in s.reqs {
            graphlava_request_free(r);
        }
        for p in s.preps {
            graphlava_prepared_free(p);
        }
        for r in s.results {
            graphlava_result_free(r);
        }
        for (db, _) in s.dbs {
            check(graphlava_close(db));
        }
    }
    let _ = std::fs::remove_file(dir().join("f.glv"));
    let _ = std::fs::remove_file(dir().join("f.journal"));
});
