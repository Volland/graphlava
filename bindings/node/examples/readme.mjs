// Every snippet of bindings/node/README.md, run end to end: `node examples/readme.mjs`.
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { Database, GraphlavaError, iri } from '../index.mjs';

const path = join(mkdtempSync(join(tmpdir(), 'glv-readme-')), 'memory.glv');

// Quick start
const db = await Database.open(path, { prefixes: { '': 'http://example.org/' } });
const t = await db.submit([{ op: 'insert', s: ':alice', p: ':worksAt', o: iri(':acme') }], 'ordered');
const works = t.results[0];                                   // a bigint statement ID
await db.submit([{ op: 'annotate', id: works, p: ':confidence', o: 0.9 }]);
await db.submit([{ op: 'update', id: works, o: iri(':globex') }]); // same ID, annotation kept

const rows = await db.sparql(`
  SELECT ?org ?c WHERE { << :alice :worksAt ?org ~ ?r >> . ?r :confidence ?c }`);
const all = await rows.toArray();
assert.equal(all.length, 1);
assert.equal(all[0].c, 0.9);

// SPARQL update and Datalog
await db.sparqlUpdate('INSERT DATA { :a :knows :b . :b :knows :c }');
const reach = await db.datalog(`
  reach(?x, ?y) :- :knows(?x, ?y) .
  reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
  ?- reach(:a, ?who) .`);
let n = 0;
for await (const _row of reach) n++;
assert.equal(n, 2);

// Batches of IDs
for await (const batch of (await db.sparql('SELECT ?s WHERE { ?s :knows ?o }')).batches()) {
  const ids = batch.ids('s');                                  // BigUint64Array, up to 1024 rows
  assert.ok(ids.length > 0);
}

// Interactive transaction
{
  await using w = await db.write('sync');
  await w.insert(':bob', ':knows', iri(':alice'));
  await w.commit();
}

// Errors
try {
  await db.submit([{ op: 'nest', child: ':g1', parent: ':g2' }, { op: 'nest', child: ':g2', parent: ':g1' }]);
  assert.fail('expected a cycle');
} catch (e) {
  assert.ok(e instanceof GraphlavaError);
  assert.equal(e.code, 'GRAPH_CYCLE');
}

await db.close();
console.log('README examples ran');
