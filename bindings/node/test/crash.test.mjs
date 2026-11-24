// SIGKILL after an acknowledged sync commit ([[21-e2e-tests]]).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Database } from '../index.mjs';
import { tempDb, PREFIXES, runNode, INDEX } from '../test-support/helpers.mjs';

// @tg: verifies:: [[openspec:node-binding#Interactive write transactions in Node#Node process killed after a sync commit]] {level: crash}
test('a Node process killed after a sync commit keeps the statement', { skip: process.platform === 'win32' && 'no SIGKILL' }, async () => {
  const { file } = tempDb();
  const child = `
    import { Database, iri } from ${JSON.stringify(INDEX)};
    const db = await Database.open(${JSON.stringify(file)}, { prefixes: { '': 'http://ex.org/' } });
    const t = await db.submit([{ op: 'insert', s: ':crash', p: ':p', o: iri(':survivor') }], 'sync');
    process.stdout.write('ID ' + t.results[0] + '\\n');
    // Keep a relaxed batch open so the kill lands mid-batch.
    await db.submit([{ op: 'insert', s: ':crash', p: ':q', o: 1 }], 'relaxed');
    setInterval(() => {}, 1000);
  `;
  let id = null;
  const res = await runNode(child, {
    onStdout: (out, proc) => {
      const m = /ID (\d+)/.exec(out);
      if (m && id === null) {
        id = BigInt(m[1]);
        setTimeout(() => proc.kill('SIGKILL'), 20);
      }
    },
  });
  assert.equal(res.signal, 'SIGKILL', res.stderr);
  assert.notEqual(id, null);
  const db = await Database.open(file, PREFIXES);
  assert.notEqual(await db.stmt(id), null);
  assert.equal(await db.checkInvariants(), null);
  await db.close();
});
