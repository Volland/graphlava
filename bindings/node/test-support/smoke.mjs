// Install smoke test: run from a project where `npm install <packed tarball>`
// installed @graphlava/node (CI, every target × Node 20/22/24). Exercises ESM,
// CommonJS, the type mapping and the agent memory session; exits non-zero on
// any failure.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';

const spec = process.env.GRAPHLAVA_SPEC || '@graphlava/node';
const require = createRequire(path.join(process.cwd(), 'noop.js'));

// @tg: verifies:: [[openspec:node-binding#Node packaging#Install and smoke test per target]] {level: e2e}
async function smoke() {
  const esm = await import(spec);
  const cjs = require(spec);
  assert.equal(esm.Database, cjs.Database);
  const { Database, iri } = esm;
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'graphlava-smoke-'));
  const file = path.join(dir, 'memory.glv');
  const opts = { prefixes: { '': 'http://ex.org/' } };
  let db = await Database.open(file, opts);
  const t = await db.submit([{ op: 'insert', s: ':alice', p: ':knows', o: iri(':bob') }], 'ordered');
  const stmt = t.results[0];
  assert.equal(typeof stmt, 'bigint');
  await db.submit([{ op: 'annotate', id: stmt, p: ':confidence', o: 0.9 }], 'ordered');
  await db.close();
  db = await Database.open(file, opts);
  const rows = await (await db.sparql('SELECT ?c WHERE { << :alice :knows :bob >> :confidence ?c }')).toArray();
  assert.deepEqual(rows, [{ c: 0.9 }]);
  assert.notEqual(await db.stmt(stmt), null);
  await db.close();
  const sync = await import(`${spec}/sync`);
  assert.equal(typeof sync.Database.openSync, 'function');
  console.log(`graphlava ${esm.version()} smoke test passed on ${process.platform}-${process.arch}, Node ${process.version}`);
}

await smoke();
