// Node API parity ([[openspec:node-binding#Node API parity]]).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Database, GraphlavaError, iri } from '../index.mjs';
import { tempDb, PREFIXES } from '../test-support/helpers.mjs';

// @tg: verifies:: [[openspec:node-binding#Node API parity#Agent memory session from Node]] {level: e2e}
// @tg: verifies:: [[21-e2e-tests#Node agent memory session]] {level: e2e}
test('agent memory session from Node', async () => {
  const { file } = tempDb('memory.glv');
  let db = await Database.open(file, PREFIXES);
  const t = await db.submit([{ op: 'insert', s: ':alice', p: ':knows', o: iri(':bob') }], 'ordered');
  const stmt = t.results[0];
  assert.equal(typeof stmt, 'bigint');
  await db.submit([{ op: 'annotate', id: stmt, p: ':confidence', o: 0.9 }], 'ordered');
  await db.close();

  db = await Database.open(file, PREFIXES);
  const rows = await db.sparql(
    'SELECT ?r ?c WHERE { ?r <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> <<( :alice :knows :bob )>> ; :confidence ?c }',
  );
  const all = await rows.toArray();
  assert.equal(all.length, 1);
  assert.equal(all[0].c, 0.9);
  // The reifier is the statement: its ID column holds the bigint from submit().
  const again = await db.sparql(
    'SELECT ?r WHERE { ?r <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> <<( :alice :knows :bob )>> ; :confidence ?c }',
  );
  const ids = [];
  for await (const b of again.batches()) ids.push(...b.ids('r'));
  assert.deepEqual(ids, [stmt]);
  assert.equal(all[0].r.iri, `https://graphlava.org/ns/stmt/${stmt}`);
  const triple = await db.stmt(stmt);
  assert.equal(triple.s, await db.lookup(iri(':alice')));
  await db.close();
});

// @tg: verifies:: [[openspec:node-binding#Node API parity#Malformed core operation]] {level: unit}
test('malformed core operation rejects INVALID naming the field and writes nothing', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const before = await db.commitCounter();
  const p = db.submit([{ op: 'insert', s: ':a', p: ':b' }], 'sync');
  assert.ok(p instanceof Promise);
  await assert.rejects(p, (e) => {
    assert.ok(e instanceof GraphlavaError);
    assert.equal(e.code, 'INVALID');
    assert.match(e.message, /`o`/);
    return true;
  });
  assert.equal(await db.commitCounter(), before);
  assert.deepEqual(await db.find(), []);
  await db.close();
});

test('every core operation is reachable as a tagged object', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const { results } = await db.submit([
    { op: 'declarePredicate', p: ':tag', multi: true },
    { op: 'createGraph', name: ':g' },
    { op: 'insert', s: ':a', p: ':knows', o: iri(':b'), graph: ':g' },
    { op: 'set', s: ':a', p: ':age', o: 30 },
    { op: 'statementId', s: ':a', p: ':age', o: 30 },
    { op: 'insertUnasserted', s: ':x', p: ':y', o: iri(':z') },
    { op: 'nest', child: ':g2', parent: ':g' },
    { op: 'declareHedgeType', type: ':Meeting', roles: [':who', { role: ':where', required: false }] },
  ]);
  const [, g, st, , ageSt, unasserted, nested, ty] = results;
  assert.equal(typeof g, 'bigint');
  assert.equal(typeof st, 'bigint');
  assert.equal(typeof ageSt, 'bigint');
  assert.equal(await db.isUnasserted(unasserted), true);
  assert.equal(nested, true);
  assert.equal(typeof ty, 'bigint');
  const more = await db.submit([
    { op: 'update', id: ageSt, o: 31, expected: 30 },
    { op: 'annotate', id: st, p: ':since', o: new Date('2024-01-02T03:04:05.678Z') },
    { op: 'addMember', item: st, graph: ':g2' },
    { op: 'membershipId', item: st, graph: ':g2' },
    { op: 'moveMember', item: st, from: ':g2', to: ':g3' },
    { op: 'removeMember', item: st, graph: ':g3' },
    { op: 'createHedge', type: ':Meeting', roles: { ':who': iri(':a') } },
    { op: 'unnest', child: ':g2', parent: ':g' },
  ]);
  const hedge = more.results[6];
  assert.equal(more.results[0], undefined);
  const h = await db.hedge(hedge);
  assert.equal(h.id, hedge);
  const role = await db.submit([
    { op: 'roleId', hedge, role: ':who', node: iri(':a') },
    { op: 'updateRole', hedge, role: ':who', filler: iri(':c') },
  ]);
  assert.equal(typeof role.results[0], 'bigint');
  const r = await db.read();
  const a = await r.lookup(iri(':a'));
  const age = await r.lookup(iri(':age'));
  const props = await r.props(a);
  const ageMatch = props.find((m) => m.p === age);
  assert.equal(await r.decode(ageMatch.o), 31);
  assert.ok((await r.out(a)).length >= 1);
  await r.close();
  const del = await db.submit([
    { op: 'remove', s: ':a', p: ':age', o: 31 },
    { op: 'dropGraph', graph: ':g2' },
    { op: 'delete', id: hedge, mode: 'detach' },
  ]);
  assert.equal(del.results[0], true);
  assert.equal(await db.hedge(hedge), null);
  await db.close();
});

test('read snapshots, write transactions and admin are reachable', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const w = await db.write('ordered');
  const id = await w.insert(':a', ':p', 'hello');
  assert.equal(id, null); // an unpromoted inline property has no ID
  const sid = await w.run({ op: 'statementId', s: ':a', p: ':p', o: 'hello' });
  assert.equal(typeof sid, 'bigint');
  const seen = await (await w.sparql('SELECT ?o WHERE { :a :p ?o }')).toArray();
  assert.deepEqual(seen, [{ o: 'hello' }]);
  await w.commit();
  assert.equal(w.active, false);
  const rows = await (await db.datalog('q(?o) :- triple(<http://ex.org/a>, <http://ex.org/p>, ?o) .\n@output q .')).toArray();
  assert.equal(rows.length, 1);
  const up = await db.sparqlUpdate('INSERT DATA { :b :p 2 }', 'sync');
  assert.equal(up.inserted, 1);
  const admin = db.admin();
  assert.equal(typeof (await admin.sweepDue()), 'boolean');
  assert.ok((await admin.fileSize()) > 0);
  await admin.buildOrdering('pso');
  await db.close();
  assert.equal(db.isOpen, false);
  await assert.rejects(db.props(1n), { code: 'CLOSED' });
});
