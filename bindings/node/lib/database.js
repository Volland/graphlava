'use strict';
// The asynchronous API of the `@graphlava/node` package ([[16-api#Node.js binding]]).
//
// Every method that can do I/O, run a query or wait on a commit returns a
// Promise; the work runs on the addon's read pool or on the database's writer
// thread, never on the event loop. Blocking variants live only in
// `@graphlava/node/sync`.
// @tg: implements:: [[openspec:node-binding#Event loop is never blocked]]

const { load } = require('./native');
const { GraphlavaError, invalid, call } = require('./errors');
const { prefixMap, fromTerm, rawTerm } = require('./terms');
const { normOp, normOps, durability, readReq, postRead, defineReads } = require('./ops');

const native = load();

const kNative = Symbol('graphlava.native');
const kPrefixes = Symbol('graphlava.prefixes');
const kCreate = Symbol('graphlava.create');

let markAsUncloneable = null;
try {
  ({ markAsUncloneable } = require('node:worker_threads'));
} catch {
  // not available
}

// @tg: implements:: [[openspec:node-binding#Single owner across workers and processes]]
/**
 * Makes `postMessage(handle)` and `structuredClone(handle)` throw
 * `DataCloneError`: a handle belongs to the thread that opened it.
 */
function noClone(obj) {
  if (typeof markAsUncloneable === 'function') {
    markAsUncloneable(obj);
  } else {
    // Node 20: an own enumerable function-valued property cannot be cloned.
    Object.defineProperty(obj, 'graphlavaHandle', { value: function graphlavaHandle() {}, enumerable: true });
  }
}

/** Prepends the database prefixes to a SPARQL text. */
function withPrefixes(text, prefixes) {
  if (typeof text !== 'string') throw invalid('the query text must be a string');
  if (prefixes.size === 0) return text;
  let head = '';
  for (const [k, v] of prefixes) head += `PREFIX ${k}: <${v}>\n`;
  return head + text;
}

/** Prepends the database prefixes to a Datalog program as `@prefix` lines
 * (a program's own `@prefix` lines come after and override them). */
function withDatalogPrefixes(text, prefixes) {
  if (typeof text !== 'string') throw invalid('the program text must be a string');
  if (prefixes.size === 0) return text;
  let head = '';
  for (const [k, v] of prefixes) head += `@prefix ${k}: <${v}> .\n`;
  return head + text;
}

function mapResults(t) {
  return { results: t.results, batch: t.batch };
}

// ---- rows --------------------------------------------------------------------------

/** One batch of at most 1024 rows; ID columns are `BigUint64Array`s. */
class Batch {
  #rows;
  constructor(rows, vars, b) {
    this.#rows = rows;
    this.vars = vars;
    this.length = b.n;
    this.columns = {};
    vars.forEach((v, i) => {
      this.columns[v] = b.ids[i];
    });
  }
  /** The ID column of `v` (0 = unbound). */
  ids(v) {
    const c = this.columns[v];
    if (!c) throw invalid(`no column ${JSON.stringify(v)}; columns: ${this.vars.join(', ')}`);
    return c;
  }
  /** Decodes the column of `v` (lazily, on first request). */
  async values(v) {
    const ids = this.ids(v);
    const terms = await call(() => this.#rows.decode(ids));
    return terms.map(fromTerm);
  }
  /** Decodes the column of `v` to lossless term objects (`{iri}`, `{literal, datatype}`, ...). */
  async terms(v) {
    const ids = this.ids(v);
    const terms = await call(() => this.#rows.decode(ids));
    return terms.map(rawTerm);
  }
}

// @tg: implements:: [[openspec:node-binding#Node row streaming]]
/**
 * A query result. Iterate rows with `for await`, or batches of up to 1024
 * rows with `rows.batches()`. Leaving either loop early (or `close()`) ends
 * the read snapshot at once.
 */
class Rows {
  #n;
  #mat;
  constructor(token, n, mat) {
    if (token !== kCreate) throw new TypeError('Rows are created by queries');
    this.#n = n;
    this.#mat = mat;
    this.vars = n ? n.vars : mat.vars;
    this.length = n ? n.length : mat.rows.length;
    noClone(this);
  }

  async *[Symbol.asyncIterator]() {
    const vars = this.vars;
    if (this.#mat) {
      for (const r of this.#mat.rows) {
        const row = {};
        vars.forEach((v, i) => {
          row[v] = fromTerm(r[i]);
        });
        yield row;
      }
      return;
    }
    const n = this.#n;
    try {
      for (;;) {
        const b = await call(() => n.nextBatch(true));
        if (b.n === 0) return;
        for (let i = 0; i < b.n; i++) {
          const row = {};
          for (let c = 0; c < vars.length; c++) row[vars[c]] = fromTerm(b.terms[c][i]);
          yield row;
        }
      }
    } finally {
      n.release();
    }
  }

  /** Batches of up to 1024 rows with `BigUint64Array` ID columns. */
  async *batches() {
    if (this.#mat) throw invalid('batches() needs an ID result: run the query with db.sparql()/db.datalog() or a ReadTxn');
    const n = this.#n;
    try {
      for (;;) {
        const b = await call(() => n.nextBatch(false));
        if (b.n === 0) return;
        yield new Batch(n, this.vars, b);
      }
    } finally {
      n.release();
    }
  }

  /** Every row, decoded. */
  async toArray() {
    const out = [];
    for await (const r of this) out.push(r);
    return out;
  }

  /** Ends the read snapshot. */
  async close() {
    if (this.#n) this.#n.release();
  }

  async [Symbol.asyncDispose]() {
    await this.close();
  }
}

// ---- read snapshot ------------------------------------------------------------------

/** A read snapshot: every read sees the same committed state. */
class ReadTxn {
  constructor(token, n, prefixes) {
    if (token !== kCreate) throw new TypeError('use db.read()');
    this[kNative] = n;
    this[kPrefixes] = prefixes;
    noClone(this);
  }
  async sparql(text) {
    const q = withPrefixes(text, this[kPrefixes]);
    return new Rows(kCreate, await call(() => this[kNative].query('sparql', q)));
  }
  async datalog(program) {
    return new Rows(kCreate, await call(() => this[kNative].query('datalog', withDatalogPrefixes(program, this[kPrefixes]))));
  }
  /** Ends this handle's hold on the snapshot. */
  async close() {
    this[kNative].release();
  }
  async [Symbol.asyncDispose]() {
    await this.close();
  }
}
defineReads(ReadTxn.prototype, '', async (self, f, args) => {
  const req = readReq(f, args, self[kPrefixes]);
  return postRead(f, await call(() => self[kNative].read(req)));
});

// ---- write transaction ----------------------------------------------------------------

// @tg: implements:: [[openspec:node-binding#Interactive write transactions in Node]]
/**
 * An interactive write transaction. Its calls run in order on the writer
 * thread. It holds the single writer for at most the time limit (default
 * 100 ms) from the moment `write()` resolved, including time spent awaiting
 * other work; past it every call rejects with `WRITE_TIMEOUT`. Prefer
 * `db.submit(ops)`. `await using` aborts it on scope exit without `commit()`.
 */
class WriteTxn {
  constructor(token, n, prefixes) {
    if (token !== kCreate) throw new TypeError('use db.write()');
    this[kNative] = n;
    this[kPrefixes] = prefixes;
    noClone(this);
  }
  /** Whether the transaction still takes calls. */
  get active() {
    return this[kNative].isActive();
  }
  /** Runs one core operation (`{op: "insert", s, p, o}`, ...). */
  async run(op) {
    const o = normOp(op, this[kPrefixes]);
    return call(() => this[kNative].run(o));
  }
  insert(s, p, o, graph) {
    return this.run({ op: 'insert', s, p, o, graph });
  }
  set(s, p, o) {
    return this.run({ op: 'set', s, p, o });
  }
  update(id, o, expected) {
    return this.run({ op: 'update', id, o, expected });
  }
  annotate(id, p, o) {
    return this.run({ op: 'annotate', id, p, o });
  }
  delete(id, mode) {
    return this.run({ op: 'delete', id, mode });
  }
  async sparql(text) {
    const q = withPrefixes(text, this[kPrefixes]);
    return new Rows(kCreate, null, await call(() => this[kNative].query('sparql', q)));
  }
  async datalog(program) {
    return new Rows(kCreate, null, await call(() => this[kNative].query('datalog', withDatalogPrefixes(program, this[kPrefixes]))));
  }
  async sparqlUpdate(text) {
    const q = withPrefixes(text, this[kPrefixes]);
    return call(() => this[kNative].sparqlUpdate(q));
  }
  /** Commits; resolves with `{batch}` once the durability level holds. */
  async commit() {
    return call(() => this[kNative].commit());
  }
  /** Aborts; nothing is written. Idempotent. */
  async abort() {
    return call(() => this[kNative].abort());
  }
  async [Symbol.asyncDispose]() {
    if (this[kNative].isActive()) await this.abort();
  }
}
defineReads(WriteTxn.prototype, '', async (self, f, args) => {
  const req = readReq(f, args, self[kPrefixes]);
  return postRead(f, await call(() => self[kNative].read(req)));
});

// ---- admin -------------------------------------------------------------------------

/** Administration: sweep and on-demand structures. Runs on the writer thread. */
class Admin {
  #db;
  constructor(db) {
    this.#db = db;
  }
  #run(req) {
    return call(() => this.#db[kNative].admin(req));
  }
  sweepDue() {
    return this.#run({ f: 'sweepDue' });
  }
  sweep(opts = {}) {
    return this.#run({ f: 'sweep', windowMs: opts.windowMs });
  }
  buildOrdering(perm) {
    return this.#run({ f: 'buildOrdering', perm });
  }
  buildWantedOrderings(threshold = 0) {
    return this.#run({ f: 'buildWantedOrderings', threshold });
  }
  async buildValueIndex(p) {
    const { nodeTerm } = require('./terms');
    return this.#run({ f: 'buildValueIndex', p: nodeTerm(p, 'p', this.#db[kPrefixes]) });
  }
  fileSize() {
    return this.#run({ f: 'fileSize' });
  }
  planCacheStats() {
    return this.#run({ f: 'planCacheStats' });
  }
}

// ---- database ------------------------------------------------------------------------

function openOptions(o) {
  const opts = o || {};
  return {
    cacheBytes: opts.cacheBytes,
    groupWindowMs: opts.groupWindowMs,
    groupBytes: opts.groupBytes,
    writeTimeoutMs: opts.writeTimeoutMs,
    cascadeLimit: opts.cascadeLimit,
    graphDepthLimit: opts.graphDepthLimit,
    maxRelaxedRun: opts.maxRelaxedRun,
  };
}

// @tg: implements:: [[openspec:node-binding#Node API parity]]
/**
 * The owner handle of one database file. One handle per file per process:
 * a second `open` (also from a worker thread) rejects with `LOCKED`. Call
 * `close()` (or use `await using`); a handle collected without it flushes
 * its pending group commit and releases the lock in its finalizer.
 */
class Database {
  constructor(token, n, prefixes) {
    if (token !== kCreate) throw new TypeError('use Database.open(path)');
    this[kNative] = n;
    this[kPrefixes] = prefixes;
    noClone(this);
  }

  /** Opens or creates a database file. */
  static async open(path, options) {
    if (typeof path !== 'string') throw invalid('path must be a string');
    const prefixes = prefixMap(options && options.prefixes);
    const n = await call(() => native.open(path, openOptions(options)));
    return new this(kCreate, n, prefixes);
  }

  /** In-memory metadata: whether the handle is open. */
  get isOpen() {
    return this[kNative].isOpen();
  }

  /** The prefixes declared at open. */
  get prefixes() {
    return Object.fromEntries(this[kPrefixes]);
  }

  /** A read snapshot. */
  async read() {
    return new ReadTxn(kCreate, await call(() => this[kNative].snapshot()), this[kPrefixes]);
  }

  /** An interactive write transaction (time-limited; prefer `submit`). */
  async write(d) {
    const dur = durability(d);
    return new WriteTxn(kCreate, await call(() => this[kNative].beginWrite(dur)), this[kPrefixes]);
  }

  /**
   * Runs core operations as one request, all or nothing; resolves with
   * `{results, batch}` once the durability level holds.
   */
  async submit(ops, d) {
    const o = normOps(ops, this[kPrefixes]);
    const dur = durability(d);
    return mapResults(await call(() => this[kNative].submit(o, dur)));
  }

  /** A SPARQL 1.2 query on a fresh snapshot. */
  async sparql(text) {
    const q = withPrefixes(text, this[kPrefixes]);
    return new Rows(kCreate, await call(() => this[kNative].query('sparql', q)));
  }

  /** A read-only Datalog program on a fresh snapshot. */
  async datalog(program) {
    if (typeof program !== 'string') throw invalid('the program must be a string');
    return new Rows(kCreate, await call(() => this[kNative].query('datalog', withDatalogPrefixes(program, this[kPrefixes]))));
  }

  /** A SPARQL Update request as one transaction. */
  async sparqlUpdate(text, d) {
    const q = withPrefixes(text, this[kPrefixes]);
    const dur = durability(d);
    return call(() => this[kNative].sparqlUpdate(q, dur));
  }

  /** A Datalog program that writes (`@assert`), all or nothing. */
  async datalogUpdate(program, d) {
    const dur = durability(d);
    return new Rows(kCreate, null, await call(() => this[kNative].datalogUpdate(withDatalogPrefixes(program, this[kPrefixes]), dur)));
  }

  /** Administration. */
  admin() {
    return new Admin(this);
  }

  /** Commits any open batch and closes the file; `BUSY` while snapshots, rows or transactions are open. */
  async close() {
    return call(() => this[kNative].close());
  }

  async [Symbol.asyncDispose]() {
    if (this.isOpen) await this.close();
  }
}
defineReads(Database.prototype, '', async (self, f, args) => {
  const req = readReq(f, args, self[kPrefixes]);
  return postRead(f, await call(() => self[kNative].read(req)));
});

function version() {
  return native.version();
}

const _debug = Object.freeze({
  /** Read snapshots alive in this process. */
  openSnapshots: () => native.debugOpenSnapshots(),
  /** Panics in the addon; the Promise rejects with `INTERNAL`. */
  panic: () => call(() => native.debugPanic()),
});

module.exports = {
  Database,
  ReadTxn,
  WriteTxn,
  Rows,
  Batch,
  Admin,
  GraphlavaError,
  version,
  _debug,
  native,
  kNative,
  kPrefixes,
  kCreate,
  withPrefixes,
  withDatalogPrefixes,
  noClone,
};
