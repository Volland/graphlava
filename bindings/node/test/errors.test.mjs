// Errors ([[openspec:node-binding#Node errors]]).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Database, GraphlavaError, iri, _debug } from '../index.mjs';
import { tempDb, PREFIXES } from '../test-support/helpers.mjs';

// @tg: verifies:: [[openspec:node-binding#Node errors#Graph cycle error carries IDs]] {level: e2e}
test('a graph cycle rejects GRAPH_CYCLE with bigint child and parent', async () => {
  const { file } = tempDb();
  let db = await Database.open(file, PREFIXES);
  await db.submit([{ op: 'nest', child: ':g2', parent: ':g1' }], 'sync');
  const g1 = await db.graph(':g1');
  const g2 = await db.graph(':g2');
  await assert.rejects(db.submit([{ op: 'nest', child: ':g1', parent: ':g2' }], 'sync'), (e) => {
    assert.ok(e instanceof GraphlavaError);
    assert.equal(e.code, 'GRAPH_CYCLE');
    assert.equal(typeof e.child, 'bigint');
    assert.equal(typeof e.parent, 'bigint');
    assert.deepEqual(new Set([e.child, e.parent]), new Set([g1, g2]));
    return true;
  });
  await db.close();
  db = await Database.open(file, PREFIXES);
  assert.deepEqual(await db.parents(g2), [g1]);
  assert.deepEqual(await db.parents(g1), []);
  await db.close();
});

// @tg: verifies:: [[openspec:node-binding#Node errors#Nesting depth limit]] {level: e2e}
test('a 65th nesting level rejects GRAPH_DEPTH_EXCEEDED with limit 64', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const ops = [];
  for (let i = 1; i <= 64; i++) ops.push({ op: 'nest', child: `:g${i}`, parent: `:g${i - 1}` });
  await db.submit(ops, 'ordered');
  await assert.rejects(db.submit([{ op: 'nest', child: ':g65', parent: ':g64' }], 'sync'), (e) => {
    assert.equal(e.code, 'GRAPH_DEPTH_EXCEEDED');
    assert.equal(e.limit, 64);
    return true;
  });
  assert.equal(await db.graph(':g65'), null);
  await db.close();
});

test('a Rust panic rejects INTERNAL and the process survives', async () => {
  await assert.rejects(_debug.panic(), (e) => {
    assert.ok(e instanceof GraphlavaError);
    assert.equal(e.code, 'INTERNAL');
    return true;
  });
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  await db.submit([{ op: 'insert', s: ':a', p: ':p', o: iri(':b') }]);
  await db.close();
});

test('error codes for other variants', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const { results } = await db.submit([{ op: 'insert', s: ':x', p: ':v', o: iri(':one') }]);
  const st = results[0];
  await assert.rejects(db.submit([{ op: 'update', id: st, o: iri(':two'), expected: iri(':zero') }]), { code: 'CONFLICT' });
  await assert.rejects(db.submit([{ op: 'delete', id: 4611686018428436479n }]), (e) => e.code === 'NOT_FOUND' && typeof e.id === 'bigint');
  await assert.rejects(db.sparql('SELECT WHERE {'), { code: 'INVALID' });
  await assert.rejects(db.submit([], 'eventually'), { code: 'INVALID' });
  await db.submit([{ op: 'insert', s: ':y', p: ':ref', o: { id: st } }]);
  await assert.rejects(db.submit([{ op: 'delete', id: st }]), (e) => e.code === 'REFERENCED_BY_STATEMENT' && e.item === st);
  await db.close();
});
