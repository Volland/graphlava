'use strict';
// @graphlava/node/sync: blocking variants for scripts and CLIs ([[16-api#Node.js binding]]).
//
// Every `*Sync` method BLOCKS the event loop until the database work is done,
// including a `sync` commit's fsync. Use the asynchronous `@graphlava/node` entry
// point in servers and agents; use this one where nothing else runs on the
// thread. `Database` here is the asynchronous class plus the `*Sync` methods.
// @tg: implements:: [[openspec:node-binding#Event loop is never blocked]]

const base = require('./lib/database');
const terms = require('./lib/terms');
const { invalid, callSync } = require('./lib/errors');
const { normOps, durability, readReq, postRead, defineReads } = require('./lib/ops');

const { native, kNative, kPrefixes, kCreate, withPrefixes, withDatalogPrefixes, noClone } = base;

/** A query result read synchronously. */
class SyncRows {
  #n;
  #mat;
  constructor(token, n, mat) {
    if (token !== kCreate) throw new TypeError('SyncRows are created by queries');
    this.#n = n;
    this.#mat = mat;
    this.vars = n ? n.vars : mat.vars;
    this.length = n ? n.length : mat.rows.length;
    noClone(this);
  }
  *[Symbol.iterator]() {
    const vars = this.vars;
    if (this.#mat) {
      for (const r of this.#mat.rows) {
        const row = {};
        vars.forEach((v, i) => {
          row[v] = terms.fromTerm(r[i]);
        });
        yield row;
      }
      return;
    }
    const n = this.#n;
    try {
      for (;;) {
        const b = callSync(() => n.nextBatchSync(true));
        if (b.n === 0) return;
        for (let i = 0; i < b.n; i++) {
          const row = {};
          for (let c = 0; c < vars.length; c++) row[vars[c]] = terms.fromTerm(b.terms[c][i]);
          yield row;
        }
      }
    } finally {
      n.release();
    }
  }
  /** Batches of up to 1024 rows: `{length, columns, values(v)}`. */
  *batches() {
    if (this.#mat) throw invalid('batches() needs an ID result');
    const n = this.#n;
    try {
      for (;;) {
        const b = callSync(() => n.nextBatchSync(false));
        if (b.n === 0) return;
        const columns = {};
        this.vars.forEach((v, i) => {
          columns[v] = b.ids[i];
        });
        yield {
          length: b.n,
          vars: this.vars,
          columns,
          values: (v) => callSync(() => n.decodeSync(columns[v])).map(terms.fromTerm),
        };
      }
    } finally {
      n.release();
    }
  }
  toArray() {
    return [...this];
  }
  close() {
    if (this.#n) this.#n.release();
  }
  [Symbol.dispose]() {
    this.close();
  }
}

/** A read snapshot with blocking reads. */
class SyncReadTxn {
  constructor(token, n, prefixes) {
    if (token !== kCreate) throw new TypeError('use db.readSync()');
    this[kNative] = n;
    this[kPrefixes] = prefixes;
    noClone(this);
  }
  sparql(text) {
    const q = withPrefixes(text, this[kPrefixes]);
    return new SyncRows(kCreate, callSync(() => this[kNative].querySync('sparql', q)));
  }
  datalog(program) {
    return new SyncRows(kCreate, callSync(() => this[kNative].querySync('datalog', withDatalogPrefixes(program, this[kPrefixes]))));
  }
  close() {
    this[kNative].release();
  }
  [Symbol.dispose]() {
    this.close();
  }
}
defineReads(SyncReadTxn.prototype, '', (self, f, args) => {
  const req = readReq(f, args, self[kPrefixes]);
  return postRead(f, callSync(() => self[kNative].readSync(req)));
});

/** The asynchronous `Database` plus blocking `*Sync` methods. */
class Database extends base.Database {
  /** Opens a database, blocking. */
  static openSync(path, options) {
    if (typeof path !== 'string') throw invalid('path must be a string');
    const prefixes = terms.prefixMap(options && options.prefixes);
    const o = options || {};
    const n = callSync(() =>
      native.openSync(path, {
        cacheBytes: o.cacheBytes,
        groupWindowMs: o.groupWindowMs,
        groupBytes: o.groupBytes,
        writeTimeoutMs: o.writeTimeoutMs,
        cascadeLimit: o.cascadeLimit,
        graphDepthLimit: o.graphDepthLimit,
        maxRelaxedRun: o.maxRelaxedRun,
      }),
    );
    return new this(kCreate, n, prefixes);
  }
  /** A read snapshot with blocking reads. */
  readSync() {
    return new SyncReadTxn(kCreate, callSync(() => this[kNative].snapshotSync()), this[kPrefixes]);
  }
  submitSync(ops, d) {
    const o = normOps(ops, this[kPrefixes]);
    const dur = durability(d);
    const t = callSync(() => this[kNative].submitSync(o, dur));
    return { results: t.results, batch: t.batch };
  }
  sparqlSync(text) {
    const q = withPrefixes(text, this[kPrefixes]);
    return new SyncRows(kCreate, callSync(() => this[kNative].querySync('sparql', q)));
  }
  datalogSync(program) {
    return new SyncRows(kCreate, callSync(() => this[kNative].querySync('datalog', withDatalogPrefixes(program, this[kPrefixes]))));
  }
  sparqlUpdateSync(text, d) {
    const q = withPrefixes(text, this[kPrefixes]);
    const dur = durability(d);
    return callSync(() => this[kNative].sparqlUpdateSync(q, dur));
  }
  datalogUpdateSync(program, d) {
    const dur = durability(d);
    return new SyncRows(kCreate, null, callSync(() => this[kNative].datalogUpdateSync(withDatalogPrefixes(program, this[kPrefixes]), dur)));
  }
  closeSync() {
    callSync(() => this[kNative].closeSync());
  }
  [Symbol.dispose]() {
    if (this.isOpen) this.closeSync();
  }
}
defineReads(Database.prototype, 'Sync', (self, f, args) => {
  const req = readReq(f, args, self[kPrefixes]);
  return postRead(f, callSync(() => self[kNative].readSync(req)));
});

module.exports = {
  Database,
  SyncReadTxn,
  SyncRows,
  GraphlavaError: base.GraphlavaError,
  version: base.version,
  iri: terms.iri,
  bnode: terms.bnode,
  literal: terms.literal,
  double: terms.double,
  XSD: terms.XSD,
};
