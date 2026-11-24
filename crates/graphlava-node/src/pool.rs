//! Threads that run database work off the JavaScript thread
//! ([[16-api#Node.js binding]], design D2).
//!
//! Reads, queries and `open` run on a process-wide read pool sized from the
//! available parallelism (the value `os.availableParallelism()` reports), not
//! on libuv's shared pool of 4. Writes run on one writer thread per database
//! ([`crate::db`]). Each call returns a Promise backed by a [`JsDeferred`]
//! that the worker thread settles; panics are caught and settle as
//! `INTERNAL`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

use napi::bindgen_prelude::Object;
use napi::{Env, JsDeferred};

use crate::conv::Out;
use crate::error::{Fail, R};

pub(crate) type Job = Box<dyn FnOnce() + Send>;
type Resolver = Box<dyn FnOnce(Env) -> napi::Result<Out> + Send>;

/// The settle side of a Promise; settles from any thread.
pub(crate) struct Settle(JsDeferred<Out, Resolver>);

impl std::fmt::Debug for Settle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Settle")
    }
}

impl Settle {
    pub fn settle(self, r: R<Out>) {
        match r {
            Ok(v) => self.0.resolve(Box::new(move |_| Ok(v))),
            Err(f) => self.0.reject(f.into_napi()),
        }
    }
}

/// A new Promise and its settle side.
pub(crate) fn promise(env: &Env) -> napi::Result<(Settle, Object<'_>)> {
    let (d, p) = env.create_deferred::<Out, Resolver>()?;
    Ok((Settle(d), p))
}

/// Runs `f`, turning a panic into an `INTERNAL` failure.
pub(crate) fn guarded<T>(f: impl FnOnce() -> R<T>) -> R<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|p| Err(Fail::panic(p)))
}

struct Pool {
    tx: Mutex<Sender<Job>>,
}

impl Pool {
    fn new(name: &str, n: usize) -> Pool {
        let (tx, rx) = channel::<Job>();
        let rx: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(rx));
        for i in 0..n {
            let rx = Arc::clone(&rx);
            std::thread::Builder::new()
                .name(format!("graphlava-{name}-{i}"))
                .spawn(move || {
                    loop {
                        let job = {
                            let rx = rx.lock().unwrap_or_else(|p| p.into_inner());
                            rx.recv()
                        };
                        match job {
                            Ok(job) => {
                                // A job settles its own Promise; a panic in it
                                // was already caught by `guarded`.
                                let _ = catch_unwind(AssertUnwindSafe(job));
                            }
                            Err(_) => return,
                        }
                    }
                })
                .expect("spawn pool thread");
        }
        Pool { tx: Mutex::new(tx) }
    }

    fn run<'e>(
        &self,
        env: &'e Env,
        f: impl FnOnce() -> R<Out> + Send + 'static,
    ) -> napi::Result<Object<'e>> {
        let (settle, p) = promise(env)?;
        let job: Job = Box::new(move || settle.settle(guarded(f)));
        let sent = self.tx.lock().unwrap_or_else(|p| p.into_inner()).send(job);
        if let Err(e) = sent {
            // The pools never shut down; run inline rather than lose the Promise.
            (e.0)();
        }
        Ok(p)
    }
}

fn parallelism() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

fn read_pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new("read", parallelism()))
}

/// One-shot write requests wait for their group commit here, many at once,
/// so concurrent `ordered` requests share a batch; the core's writer token
/// orders them ([[06-transactions-durability#Group commit]]).
fn commit_pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Pool::new("commit", (4 * parallelism()).max(32)))
}

/// Runs a one-shot write request on the commit pool.
pub(crate) fn on_commit_pool<'e>(
    env: &'e Env,
    f: impl FnOnce() -> R<Out> + Send + 'static,
) -> napi::Result<Object<'e>> {
    commit_pool().run(env, f)
}

// @tg: implements:: [[openspec:node-binding#Event loop is never blocked]]
/// Runs `f` on the read pool and returns a Promise of its result.
pub(crate) fn on_pool<'e>(
    env: &'e Env,
    f: impl FnOnce() -> R<Out> + Send + 'static,
) -> napi::Result<Object<'e>> {
    read_pool().run(env, f)
}
