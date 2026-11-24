//! Node.js binding of graphlava ([[16-api#Node.js binding]], [[adr:0042]]).
//!
//! A napi-rs addon (N-API 9) over the Rust API. The npm package in
//! `bindings/node/` wraps these native classes in the public JavaScript API
//! (`Database`, `ReadTxn`, `WriteTxn`, `Rows`, `GraphlavaError`) and does the
//! host-value mapping of terms.
//!
//! - Every call that does I/O, runs a query or waits on a commit returns a
//!   Promise and runs off the JavaScript thread: reads on a read pool
//!   (`pool`), writes in order on one writer thread per database (`db`,
//!   `txn`). The `*Sync` methods exist only for `@graphlava/node/sync`.
//! - IDs cross as `bigint` (and `BigUint64Array` columns), never as `number`.
//! - A process-global registry of open paths refuses a second open of a file
//!   in this process, from any worker thread, with `LOCKED`.
//! - Errors carry a stable code (`error`); a panic becomes `INTERNAL`.

// @tg: implements:: [[adr:0042]]

// napi-derive leaves out the JS registration under `cfg(test)`, so the exported
// functions look unused to `cargo clippy --all-targets`.
#![cfg_attr(test, allow(dead_code))]

mod conv;
mod db;
mod error;
mod pool;
mod snapshot;
mod txn;

use napi::Env;
use napi::bindgen_prelude::Object;
use napi_derive::napi;

pub use conv::{AdminIn, OpIn, OpenIn, PairIn, ReadIn, RoleSpecIn, TermIn};
pub use db::NativeDb;
pub use snapshot::{NativeRows, NativeSnapshot};
pub use txn::NativeTxn;

/// The graphlava version this addon was built from.
#[napi]
pub fn version() -> String {
    graphlava::VERSION.to_string()
}

/// Read snapshots alive in this process (`graphlava._debug.openSnapshots()`).
#[napi]
pub fn debug_open_snapshots() -> u32 {
    snapshot::open_snapshots() as u32
}

/// Panics on a pool thread; the Promise rejects with `INTERNAL` and the
/// process survives (`graphlava._debug.panic()`).
#[napi(ts_return_type = "Promise<never>")]
pub fn debug_panic(env: &Env) -> napi::Result<Object<'_>> {
    pool::on_pool(env, || panic!("graphlava._debug.panic() was called"))
}
