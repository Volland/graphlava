//! Interactive write transactions on the writer thread
//! ([[16-api#Node.js binding]], design D3).
//!
//! `beginWrite` runs [`run_txn`] on the database's writer thread. It opens a
//! [`WriteTxn`], resolves the Promise with a [`NativeTxn`], then serves only
//! that transaction's calls, in order, until `commit()`, `abort()` or the
//! time limit. The limit runs from the moment `write()` resolves and does not
//! pause while JavaScript awaits other work: at the deadline the writer
//! thread aborts the transaction and moves on to queued requests, so one slow
//! caller cannot hold the single writer. Later calls reject with
//! `WRITE_TIMEOUT`.

use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use graphlava::{Durability, WriteTxn};
use napi::Env;
use napi::bindgen_prelude::Object;
use napi_derive::napi;

use crate::conv::{self, OpIn, Out, ReadIn, op_result, query_result_out};
use crate::db::{Live, Owned, read_call};
use crate::error::{Fail, R, write_timeout};
use crate::pool::{Settle, guarded, promise};

type TxnJob = Box<dyn for<'a> FnOnce(&mut Option<WriteTxn<'a>>, &TxnShared) + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Committed,
    Aborted,
    Timeout,
}

#[derive(Debug)]
struct Link {
    tx: Option<Sender<TxnJob>>,
    end: Option<End>,
}

#[derive(Debug)]
struct TxnShared {
    link: Mutex<Link>,
}

impl TxnShared {
    fn ended(&self) -> Fail {
        match self.link.lock().unwrap_or_else(|p| p.into_inner()).end {
            Some(End::Timeout) | None => write_timeout(),
            Some(End::Committed) => Fail::invalid("the write transaction is already committed"),
            Some(End::Aborted) => Fail::invalid("the write transaction is already aborted"),
        }
    }

    fn set_end(&self, e: End) {
        let mut l = self.link.lock().unwrap_or_else(|p| p.into_inner());
        l.tx = None;
        l.end.get_or_insert(e);
    }
}

// @tg: implements:: [[openspec:node-binding#Interactive write transactions in Node]]
/// The native side of a JS `WriteTxn`.
#[napi]
#[derive(Debug)]
pub struct NativeTxn {
    shared: Arc<TxnShared>,
}

/// The body of one transaction on the writer thread.
pub(crate) fn run_txn(owned: Arc<Owned>, d: Durability, settle: Settle) {
    let live = Live::new(&owned);
    let w = match guarded(|| Ok(live.owned().db.write(d)?)) {
        Ok(w) => w,
        Err(e) => return settle.settle(Err(e)),
    };
    let deadline = Instant::now() + owned.write_timeout;
    let (tx, rx) = channel::<TxnJob>();
    let shared = Arc::new(TxnShared {
        link: Mutex::new(Link {
            tx: Some(tx),
            end: None,
        }),
    });
    settle.settle(Ok(Out::Txn(NativeTxn {
        shared: Arc::clone(&shared),
    })));
    let mut w = Some(w);
    let end = loop {
        let now = Instant::now();
        if now >= deadline {
            break End::Timeout;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(job) => {
                job(&mut w, &shared);
                if w.is_none() {
                    // commit() or abort() consumed it; the job set the end.
                    break End::Aborted;
                }
            }
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                break End::Timeout;
            }
        }
    };
    // Dropping an open transaction aborts it.
    drop(w);
    shared.set_end(end);
    // Calls sent before the end was recorded settle now, as ended.
    while let Ok(job) = rx.try_recv() {
        job(&mut None, &shared);
    }
    drop(live);
}

impl NativeTxn {
    /// Sends one call to the writer thread; `f` gets the open transaction.
    fn call<'e>(
        &self,
        env: &'e Env,
        f: impl for<'a> FnOnce(&mut Option<WriteTxn<'a>>, &TxnShared) -> R<Out> + Send + 'static,
    ) -> napi::Result<Object<'e>> {
        let (settle, p) = promise(env)?;
        let link = self.shared.link.lock().unwrap_or_else(|p| p.into_inner());
        match &link.tx {
            Some(tx) => {
                let job: TxnJob = Box::new(move |w, sh| settle.settle(guarded(|| f(w, sh))));
                if let Err(e) = tx.send(job) {
                    drop(link);
                    (e.0)(&mut None, &self.shared);
                }
            }
            None => {
                drop(link);
                settle.settle(Err(self.shared.ended()));
            }
        }
        Ok(p)
    }

    fn with<'e>(
        &self,
        env: &'e Env,
        f: impl for<'a> FnOnce(&mut WriteTxn<'a>) -> R<Out> + Send + 'static,
    ) -> napi::Result<Object<'e>> {
        self.call(env, move |w, sh| match w.as_mut() {
            Some(w) => f(w),
            None => Err(sh.ended()),
        })
    }
}

#[napi]
impl NativeTxn {
    /// In-memory metadata: whether the transaction can still take calls.
    #[napi]
    pub fn is_active(&self) -> bool {
        self.shared
            .link
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .tx
            .is_some()
    }

    /// Runs one core operation inside the transaction.
    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn run<'e>(&self, env: &'e Env, op: OpIn) -> napi::Result<Object<'e>> {
        let op = conv::core_op(op);
        self.with(env, move |w| Ok(op_result(&w.run(&op?)?)))
    }

    /// A core read that sees this transaction's writes.
    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn read<'e>(&self, env: &'e Env, req: ReadIn) -> napi::Result<Object<'e>> {
        self.with(env, move |w| {
            let mut out = None;
            w.read(|v| {
                out = Some(read_call(v, &req));
                Ok(())
            })?;
            out.expect("read ran")
        })
    }

    /// A query that sees this transaction's writes, decoded at once.
    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn query<'e>(&self, env: &'e Env, lang: String, text: String) -> napi::Result<Object<'e>> {
        self.with(env, move |w| {
            let q = match conv::lang(&lang)? {
                graphlava::Lang::Sparql => w.sparql(&text)?,
                graphlava::Lang::Datalog => w.datalog(&text)?,
            };
            Ok(query_result_out(&q))
        })
    }

    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn sparql_update<'e>(&self, env: &'e Env, text: String) -> napi::Result<Object<'e>> {
        self.with(env, move |w| {
            let r = w.sparql_update(&text)?;
            Ok(Out::Obj(vec![
                ("inserted", Out::Num(r.inserted as f64)),
                ("deleted", Out::Num(r.deleted as f64)),
                ("updatedInPlace", Out::Num(r.updated_in_place as f64)),
            ]))
        })
    }

    /// Commits; resolves once the durability level holds.
    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn commit<'e>(&self, env: &'e Env) -> napi::Result<Object<'e>> {
        self.call(env, |w, sh| {
            let Some(t) = w.take() else {
                return Err(sh.ended());
            };
            // Set before the result reaches JS, so later calls name the end.
            let r = t.commit();
            sh.set_end(if r.is_ok() {
                End::Committed
            } else {
                End::Aborted
            });
            let t = r?;
            Ok(Out::Obj(vec![("batch", Out::Id(t.batch))]))
        })
    }

    /// Aborts; nothing of the transaction is written. Idempotent.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn abort<'e>(&self, env: &'e Env) -> napi::Result<Object<'e>> {
        if !self.is_active() {
            let (settle, p) = promise(env)?;
            settle.settle(Ok(Out::Undefined));
            return Ok(p);
        }
        self.call(env, |w, sh| {
            if let Some(t) = w.take() {
                t.abort();
                sh.set_end(End::Aborted);
            }
            Ok(Out::Undefined)
        })
    }
}
