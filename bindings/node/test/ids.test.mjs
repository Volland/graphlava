// IDs are bigint ([[openspec:node-binding#Node ID mapping]]).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Database, iri } from '../index.mjs';
import { tempDb, PREFIXES } from '../test-support/helpers.mjs';

// @tg: verifies:: [[openspec:node-binding#Node ID mapping#Tagged IDs round trip]] {level: e2e}
test('a hyperedge ID with kind tag bits set round-trips through a reopen', async () => {
  const { file } = tempDb();
  let db = await Database.open(file, PREFIXES);
  await db.submit([{ op: 'declareHedgeType', type: ':Meeting', roles: [':who', ':where'] }]);
  const t = await db.submit([
    { op: 'createHedge', type: ':Meeting', roles: [[':who', iri(':alice')], [':where', iri(':office')]] },
  ]);
  const id = t.results[0];
  assert.equal(typeof id, 'bigint');
  assert.ok(id >> 60n !== 0n, `kind tag of ${id.toString(16)}`);
  assert.ok(id > BigInt(Number.MAX_SAFE_INTEGER));
  assert.ok(id < 2n ** 64n);
  await db.close();
  db = await Database.open(file, PREFIXES);
  const h = await db.hedge(id);
  assert.equal(h.id, id);
  assert.equal(typeof h.type, 'bigint');
  assert.equal(h.roles.length, 2);
  for (const [role, filler] of h.roles) {
    assert.equal(typeof role, 'bigint');
    assert.equal(typeof filler, 'bigint');
  }
  assert.deepEqual(await db.decode(h.type), { iri: 'http://ex.org/Meeting' });
  // Row ID columns carry the same full 64-bit values.
  const rows = await db.sparql('SELECT ?s WHERE { ?s :who ?x }');
  for await (const b of rows.batches()) assert.equal(b.ids('s')[0], id);
  await db.close();
});

// @tg: verifies:: [[openspec:node-binding#Node ID mapping#Number passed as an ID]] {level: unit}
test('a number passed as an ID rejects INVALID and asks for a bigint', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const p = db.stmt(42);
  assert.ok(p instanceof Promise);
  await assert.rejects(p, (e) => {
    assert.equal(e.code, 'INVALID');
    assert.match(e.message, /bigint/);
    return true;
  });
  await assert.rejects(db.submit([{ op: 'delete', id: 7 }]), { code: 'INVALID' });
  await assert.rejects(db.stmt(-1n), { code: 'INVALID' });
  await assert.rejects(db.stmt(2n ** 64n), { code: 'INVALID' });
  assert.equal(await db.stmt(2n ** 64n - 1n), null);
  await db.close();
});
