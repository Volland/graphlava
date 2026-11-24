// Row streaming ([[openspec:node-binding#Node row streaming]]).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Database, iri, _debug } from '../index.mjs';
import { tempDb, PREFIXES } from '../test-support/helpers.mjs';

async function seed(db, n) {
  const ops = [];
  for (let i = 0; i < n; i++) ops.push({ op: 'insert', s: `:r${i}`, p: ':k', o: iri(`:v${i}`) });
  await db.submit(ops, 'ordered');
}

// @tg: verifies:: [[openspec:node-binding#Node row streaming#Typed-array batches]] {level: e2e}
test('3,000 rows arrive as batches of 1024, 1024 and 952 with BigUint64Array columns', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  await seed(db, 3000);
  const rows = await db.sparql('SELECT ?s ?o WHERE { ?s :k ?o }');
  assert.equal(rows.length, 3000);
  const sizes = [];
  let decoded = 0;
  for await (const b of rows.batches()) {
    sizes.push(b.length);
    assert.ok(b.columns.s instanceof BigUint64Array);
    assert.ok(b.ids('o') instanceof BigUint64Array);
    assert.equal(b.ids('s').length, b.length);
    if (decoded === 0) {
      // Terms are decoded only for the column asked for.
      const vals = await b.values('o');
      assert.equal(vals.length, b.length);
      assert.ok(vals[0].iri.startsWith('http://ex.org/v'));
      decoded++;
    }
  }
  assert.deepEqual(sizes, [1024, 1024, 952]);
  assert.equal(_debug.openSnapshots(), 0);
  await db.close();
});

// @tg: verifies:: [[openspec:node-binding#Node row streaming#Early break releases the snapshot]] {level: e2e}
test('breaking out after the first of 10,000 rows releases the snapshot', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  await seed(db, 10000);
  assert.equal(_debug.openSnapshots(), 0);
  const rows = await db.sparql('SELECT ?s WHERE { ?s :k ?o }');
  assert.equal(_debug.openSnapshots(), 1);
  let seen = 0;
  for await (const row of rows) {
    assert.ok(row.s.iri);
    seen++;
    break;
  }
  assert.equal(seen, 1);
  assert.equal(_debug.openSnapshots(), 0);
  // The database closes: no handle holds it.
  await db.close();
});

test('close() is BUSY while rows hold a snapshot', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  await seed(db, 3);
  const rows = await db.sparql('SELECT ?s WHERE { ?s :k ?o }');
  await assert.rejects(db.close(), { code: 'BUSY' });
  assert.equal(db.isOpen, true);
  await rows.close();
  await db.close();
});

test('ASK and CONSTRUCT results are rows too', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  await seed(db, 2);
  assert.deepEqual(await (await db.sparql('ASK { :r0 :k :v0 }')).toArray(), [{ ask: true }]);
  const g = await (await db.sparql('CONSTRUCT { ?s :copy ?o } WHERE { ?s :k ?o }')).toArray();
  assert.equal(g.length, 2);
  assert.deepEqual(Object.keys(g[0]), ['s', 'p', 'o']);
  await db.close();
});
