// Host values ↔ XSD literals ([[openspec:node-binding#Node literal mapping]]).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { Database, iri, double } from '../index.mjs';
import { tempDb, PREFIXES } from '../test-support/helpers.mjs';

async function readBack(db, subject) {
  const s = await db.lookup(iri(subject));
  const p = await db.lookup(iri(':n'));
  const ms = await db.out(s, p);
  assert.equal(ms.length, 1);
  return db.decode(ms[0].o);
}

// @tg: verifies:: [[openspec:node-binding#Node literal mapping#Integers across the safe and inline limits]] {level: e2e}
test('integers across the safe and inline limits', async () => {
  const { file } = tempDb();
  const values = [2n ** 53n - 1n, 2n ** 57n - 1n, -(2n ** 57n), 2n ** 57n];
  let db = await Database.open(file, PREFIXES);
  await db.submit(values.map((v, i) => ({ op: 'insert', s: `:x${i}`, p: ':n', o: v })), 'ordered');
  await db.close();
  db = await Database.open(file, PREFIXES);
  const got = [];
  for (let i = 0; i < values.length; i++) got.push(await readBack(db, `:x${i}`));
  assert.equal(typeof got[0], 'number');
  assert.equal(got[0], 9007199254740991);
  for (let i = 1; i < values.length; i++) {
    assert.equal(typeof got[i], 'bigint', `value ${i}`);
    assert.equal(got[i], values[i]);
  }
  await db.close();
});

// @tg: verifies:: [[openspec:node-binding#Node literal mapping#Unsafe number is rejected]] {level: unit}
test('an unsafe-integer number is rejected with a hint to pass a bigint', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  await assert.rejects(db.submit([{ op: 'insert', s: ':a', p: ':n', o: 2 ** 60 }]), (e) => {
    assert.equal(e.code, 'INVALID');
    assert.match(e.message, /bigint/);
    return true;
  });
  await assert.rejects(db.submit([{ op: 'insert', s: ':a', p: ':n', o: NaN }]), { code: 'INVALID' });
  assert.deepEqual(await db.find(), []);
  // The explicit escape hatches.
  await db.submit([{ op: 'insert', s: ':a', p: ':n', o: 2n ** 60n }, { op: 'insert', s: ':b', p: ':n', o: double(2 ** 60) }]);
  assert.equal(await readBack(db, ':a'), 2n ** 60n);
  assert.equal(await readBack(db, ':b'), 2 ** 60);
  await db.close();
});

test('host values map to XSD datatypes and back', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  const d = new Date('2026-11-24T09:00:00.123Z');
  const cases = [
    ['hello', 'hello'],
    [true, true],
    [false, false],
    [-0, 0],
    [0.5, 0.5],
    [Infinity, Infinity],
    [-Infinity, -Infinity],
    [d, d],
    [2n ** 70n, 2n ** 70n],
    [{ value: 'chat', lang: 'fr' }, { value: 'chat', lang: 'fr' }],
    [{ value: '1.50', datatype: 'http://www.w3.org/2001/XMLSchema#decimal' }, { value: '1.5', datatype: 'http://www.w3.org/2001/XMLSchema#decimal' }],
    [iri(':bob'), { iri: 'http://ex.org/bob' }],
  ];
  await db.submit(cases.map(([v], i) => ({ op: 'insert', s: `:c${i}`, p: ':n', o: v })));
  for (let i = 0; i < cases.length; i++) assert.deepEqual(await readBack(db, `:c${i}`), cases[i][1], `case ${i}`);
  await db.close();
});

test('property: safe integers, doubles, bigints and strings round-trip', async () => {
  const { file } = tempDb();
  const db = await Database.open(file, PREFIXES);
  let n = 0;
  await fc.assert(
    fc.asyncProperty(
      fc.oneof(
        fc.maxSafeInteger(),
        fc.double({ noNaN: true }).filter((x) => !Number.isInteger(x)),
        fc.bigInt({ min: -(2n ** 80n), max: 2n ** 80n }),
        fc.string(),
      ),
      async (v) => {
        const s = `:p${n++}`;
        await db.submit([{ op: 'insert', s, p: ':n', o: v }], 'relaxed');
        const got = await readBack(db, s);
        if (typeof v === 'bigint') {
          const safe = v >= -BigInt(Number.MAX_SAFE_INTEGER) && v <= BigInt(Number.MAX_SAFE_INTEGER);
          assert.equal(got, safe ? Number(v) : v);
        } else if (typeof v === 'number' && Object.is(v, -0)) {
          assert.equal(got, 0);
        } else {
          assert.ok(Object.is(got, v) || got === v, `${String(v)} came back as ${String(got)}`);
        }
      },
    ),
    { numRuns: 300 },
  );
  await db.close();
});
