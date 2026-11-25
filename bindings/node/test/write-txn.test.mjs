// Interactive write transactions ([[openspec:node-binding#Interactive write transactions in Node]]).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Database, iri } from '../index.mjs';
import { tempDb, PREFIXES, sleep } from '../test-support/helpers.mjs';

// `await using` is syntax: Node 24+ parses it; older Node runs the same body
// through the explicit Symbol.asyncDispose protocol.
let awaitUsing = true;
try {
  new Function('async function f(x) { await using y = x; }');
} catch {
  awaitUsing = false;
}

// @tg: verifies:: [[openspec:node-binding#Interactive write transactions in Node#Scope exit without commit aborts]] {level: e2e}
test('scope exit without commit aborts', async () => {
  const { file } = tempDb();
  let db = await Database.open(file, PREFIXES);
  const boom = new Error('boom');
  const body = awaitUsing
    ? (await import('../test-support/await-using.mjs')).insertThreeThenThrow
    : async (db, err) => {
        const w = await db.write('sync');
        try {
          await w.insert(':a', ':p', iri(':x1'));
          await w.insert(':a', ':p', iri(':x2'));
          await w.insert(':a', ':p', iri(':x3'));
          throw err;
        } finally {
          await w[Symbol.asyncDispose]();
        }
      };
  await assert.rejects(body(db, boom), (e) => e === boom);
  await db.close();
  db = await Database.open(file, PREFIXES);
  assert.equal(await db.lookup(iri(':x1')), null);
  assert.equal((await (await db.sparql('SELECT * WHERE { :a :p ?o }')).toArray()).length, 0);
  // The writer is free again.
  await db.submit([{ op: 'insert', s: ':a', p: ':p', o: iri(':y') }], 'sync');
  await db.close();
});

// @tg: verifies:: [[openspec:node-binding#Interactive write transactions in Node#Time limit across awaits]] {level: e2e}
test('the time limit runs across awaits', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const w = await db.write('sync');
  const concurrent = db.submit([{ op: 'insert', s: ':other', p: ':p', o: iri(':o') }], 'sync');
  await sleep(150);
  await assert.rejects(w.insert(':a', ':p', iri(':late')), (e) => {
    assert.equal(e.code, 'WRITE_TIMEOUT');
    assert.match(e.message, /submit/);
    return true;
  });
  const t = await concurrent;
  assert.equal(typeof t.results[0], 'bigint');
  assert.notEqual(await db.stmt(t.results[0]), null);
  await assert.rejects(w.commit(), { code: 'WRITE_TIMEOUT' });
  await w.abort(); // idempotent after the deadline
  await db.close();
});

test('commit makes the writes durable and ends the transaction', async () => {
  const { file } = tempDb();
  let db = await Database.open(file, PREFIXES);
  const w = await db.write('sync');
  await w.insert(':a', ':p', iri(':b'));
  const sid = await w.run({ op: 'statementId', s: ':a', p: ':p', o: iri(':b') });
  assert.notEqual(await w.stmt(sid), null); // read-your-writes
  const { batch } = await w.commit();
  assert.equal(typeof batch, 'bigint');
  await assert.rejects(w.insert(':a', ':p', iri(':c')), { code: 'INVALID' });
  await w[Symbol.asyncDispose]();
  await db.close();
  db = await Database.open(file, PREFIXES);
  assert.notEqual(await db.stmt(sid), null);
  await db.close();
});
