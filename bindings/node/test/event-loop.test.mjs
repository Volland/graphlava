// The event loop is never blocked ([[openspec:node-binding#Event loop is never blocked]]).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as graphlava from '../index.mjs';
import * as graphlavaSync from '../sync.mjs';
import { tempDb, PREFIXES } from '../test-support/helpers.mjs';

const { Database } = graphlava;

// @tg: verifies:: [[openspec:node-binding#Event loop is never blocked#Timers keep firing during a sync commit]] {level: e2e}
test('timers keep firing during 200 sync commits', async () => {
  const { file } = tempDb();
  let db = await Database.open(file, PREFIXES);
  const gaps = [];
  let last = performance.now();
  const timer = setInterval(() => {
    const now = performance.now();
    gaps.push(now - last);
    last = now;
  }, 1);
  const ids = [];
  try {
    for (let i = 0; i < 200; i++) {
      const t = await db.submit([{ op: 'insert', s: `:n${i}`, p: ':seq', o: graphlava.iri(`:v${i}`) }], 'sync');
      ids.push(t.results[0]);
    }
  } finally {
    clearInterval(timer);
  }
  gaps.sort((a, b) => a - b);
  const p99 = gaps[Math.min(gaps.length - 1, Math.floor(gaps.length * 0.99))];
  assert.ok(gaps.length > 10, `the interval fired ${gaps.length} times`);
  assert.ok(p99 < 20, `p99 gap between interval callbacks is ${p99.toFixed(2)} ms`);
  await db.close();
  db = await Database.open(file, PREFIXES);
  for (const id of ids) assert.notEqual(await db.stmt(id), null);
  await db.close();
});

// @tg: verifies:: [[openspec:node-binding#Event loop is never blocked#Sync entry point is opt-in]] {level: unit}
test('blocking variants exist only on @graphlava/node/sync', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  assert.equal(db.readSync, undefined);
  assert.equal(db.submitSync, undefined);
  for (const k of Object.getOwnPropertyNames(Database.prototype)) assert.ok(!k.endsWith('Sync'), k);
  // Every I/O method of the default entry point returns a Promise.
  const p = db.props(1n);
  assert.ok(p instanceof Promise);
  await p;
  await db.close();

  const { file: file2 } = tempDb();
  const sdb = graphlavaSync.Database.openSync(file2, PREFIXES);
  assert.equal(typeof sdb.readSync, 'function');
  const t = sdb.submitSync([{ op: 'insert', s: ':a', p: ':p', o: 1 }], 'sync');
  assert.deepEqual(t.results, [null]);
  const r = sdb.readSync();
  const a = r.lookup(graphlava.iri(':a'));
  assert.equal(r.props(a).length, 1);
  assert.deepEqual(r.sparql('SELECT ?o WHERE { :a :p ?o }').toArray(), [{ o: 1 }]);
  r.close();
  sdb.closeSync();
  // In-memory metadata stays synchronous on the default entry point.
  assert.equal(typeof graphlava.version(), 'string');
  assert.equal(sdb.isOpen, false);
});
