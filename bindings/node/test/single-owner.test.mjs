// One owner per file ([[openspec:node-binding#Single owner across workers and processes]]).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Worker } from 'node:worker_threads';
import { Database, iri } from '../index.mjs';
import { tempDb, PREFIXES, runNode, INDEX } from '../test-support/helpers.mjs';

function inWorker(source) {
  return new Promise((resolve, reject) => {
    const w = new Worker(source, { eval: true, type: 'module' });
    w.once('message', (m) => {
      resolve(m);
      w.terminate();
    });
    w.once('error', reject);
  });
}

// @tg: verifies:: [[openspec:node-binding#Single owner across workers and processes#Worker thread is refused]] {level: e2e}
test('a worker thread cannot open a file the main thread holds', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const src = `
    import { parentPort } from 'node:worker_threads';
    const { Database } = await import(${JSON.stringify(INDEX)});
    try {
      const db = await Database.open(${JSON.stringify(file)});
      await db.close();
      parentPort.postMessage({ ok: true });
    } catch (e) {
      parentPort.postMessage({ ok: false, code: e.code, name: e.name });
    }
  `;
  const res = await inWorker(src);
  assert.deepEqual(res, { ok: false, code: 'LOCKED', name: 'GraphlavaError' });
  const t = await db.submit([{ op: 'insert', s: ':a', p: ':p', o: iri(':b') }], 'sync');
  assert.equal(typeof t.results[0], 'bigint');
  await db.close();
  // After close, the worker may open it.
  const again = await inWorker(src);
  assert.deepEqual(again, { ok: true });
});

// @tg: verifies:: [[openspec:node-binding#Single owner across workers and processes#Close releases the lock]] {level: e2e}
test('close releases the lock for another process', async () => {
  const { file } = tempDb();
  const first = await runNode(`
    import { Database, iri } from ${JSON.stringify(INDEX)};
    const db = await Database.open(${JSON.stringify(file)}, { prefixes: { '': 'http://ex.org/' } });
    const t = await db.submit([{ op: 'insert', s: ':a', p: ':p', o: iri(':b') }], 'ordered');
    await db.close();
    console.log('ID ' + t.results[0]);
  `);
  assert.equal(first.code, 0, first.stderr);
  const id = BigInt(/ID (\d+)/.exec(first.stdout)[1]);
  const second = await runNode(`
    import { Database } from ${JSON.stringify(INDEX)};
    const db = await Database.open(${JSON.stringify(file)});
    console.log('SEEN ' + ((await db.stmt(${id}n)) !== null));
    await db.close();
  `);
  assert.equal(second.code, 0, second.stderr);
  assert.match(second.stdout, /SEEN true/);
});

test('another process gets LOCKED while the file is held', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const other = await runNode(`
    import { Database } from ${JSON.stringify(INDEX)};
    try { await Database.open(${JSON.stringify(file)}); console.log('OPENED'); }
    catch (e) { console.log('CODE ' + e.code); }
  `);
  assert.match(other.stdout, /CODE LOCKED/, other.stderr);
  await db.close();
});

test('a second open in the same thread is refused, and close frees it', async () => {
  const { file } = tempDb();
  const db = await Database.open(file);
  await assert.rejects(Database.open(file), { code: 'LOCKED' });
  await db.close();
  const db2 = await Database.open(file);
  await db2.close();
});

test('a collected handle flushes and releases the lock in its finalizer', async () => {
  const { file } = tempDb();
  const res = await runNode(
    `
    import { Database, iri } from ${JSON.stringify(INDEX)};
    const path = ${JSON.stringify(file)};
    let id;
    await (async () => {
      const db = await Database.open(path, { prefixes: { '': 'http://ex.org/' }, groupWindowMs: 60000 });
      const t = await db.submit([{ op: 'insert', s: ':gc', p: ':p', o: iri(':o') }], 'relaxed');
      id = t.results[0];
    })();
    for (let i = 0; i < 50; i++) {
      globalThis.gc();
      await new Promise((r) => setTimeout(r, 20));
      try {
        const db = await Database.open(path);
        console.log('REOPENED ' + ((await db.stmt(id)) !== null));
        await db.close();
        process.exit(0);
      } catch (e) {
        if (e.code !== 'LOCKED') throw e;
      }
    }
    console.log('STILL LOCKED');
  `,
    { args: ['--expose-gc'] },
  );
  assert.match(res.stdout, /REOPENED true/, res.stdout + res.stderr);
});

// @tg: verifies:: [[openspec:node-binding#Single owner across workers and processes#Posting a handle to a worker]] {level: unit}
test('posting a handle to a worker throws DataCloneError', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const worker = new Worker('setInterval(() => {}, 1000)', { eval: true });
  try {
    assert.throws(() => worker.postMessage(db), (e) => {
      assert.equal(e.name, 'DataCloneError');
      return true;
    });
    const r = await db.read();
    assert.throws(() => worker.postMessage(r), { name: 'DataCloneError' });
    await r.close();
  } finally {
    await worker.terminate();
  }
  const t = await db.submit([{ op: 'insert', s: ':a', p: ':p', o: iri(':b') }], 'sync');
  assert.equal(typeof t.results[0], 'bigint');
  await db.close();
});
