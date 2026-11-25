// Node runner of the shared binding conformance corpus (bindings/conformance/,
// [[adr:0042]]): every case whose `hosts` include "*" or "node".
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { Database, double, GraphlavaError } from '../index.mjs';
import { tempDb, PKG } from '../test-support/helpers.mjs';

const DIR = path.resolve(PKG, '../conformance');
const EX = 'http://ex.org/';
const OPTS = { prefixes: { ex: EX } };

const cases = fs
  .readdirSync(DIR)
  .filter((f) => f.endsWith('.jsonl'))
  .sort()
  .flatMap((f) =>
    fs
      .readFileSync(path.join(DIR, f), 'utf8')
      .split('\n')
      .filter((l) => l.trim())
      .map((l) => JSON.parse(l)),
  )
  .filter((c) => (c.hosts || ['*']).some((h) => h === '*' || h === 'node'));

const code = (variant) => variant.replace(/([a-z])([A-Z])/g, '$1_$2').toUpperCase();

/** A corpus term as a Node input term. */
function hostTerm(t, names) {
  if ('iri' in t) return { iri: t.iri };
  if ('bnode' in t) return { bnode: t.bnode };
  if ('ref' in t) return { id: names.get(t.ref) };
  if ('literal' in t) return t.lang !== undefined ? { value: t.literal, lang: t.lang } : { value: t.literal, datatype: t.datatype };
  throw new Error(`unsupported input term ${JSON.stringify(t)}`);
}

const SKIP = Symbol('skip');
const parseNumber = (v) => (v === 'INF' || v === 'Infinity' ? Infinity : v === '-INF' || v === '-Infinity' ? -Infinity : Number(v));
const MS = /\.\d{4,}/; // more than millisecond precision does not fit a Date

/** The Node host value of a native corpus value, or SKIP when Node has no such type. */
function hostNative(n) {
  switch (n.type) {
    case 'int':
    case 'bigint':
      return BigInt(n.value);
    case 'float': {
      const x = parseNumber(n.value);
      // JS has one number type: an integral float is written with double().
      return Number.isNaN(x) || !(Number.isInteger(x) || Object.is(x, -0)) ? x : double(x);
    }
    case 'number':
      return parseNumber(n.value);
    case 'bool':
      return Boolean(n.value);
    case 'str':
      return String(n.value);
    case 'datetime':
    case 'date_ms':
      return MS.test(n.value) ? SKIP : new Date(n.value);
    default:
      return SKIP; // decimal, date, naive_datetime: no Node host type
  }
}

/** Checks a decoded host value against a native expectation. */
function checkNative(got, n, at) {
  switch (n.type) {
    case 'int':
      assert.ok(typeof got === 'number' || typeof got === 'bigint', `${at}: ${typeof got}`);
      assert.equal(BigInt(got), BigInt(n.value), at);
      return;
    case 'bigint':
      assert.equal(typeof got, 'bigint', at);
      assert.equal(got, BigInt(n.value), at);
      return;
    case 'number':
    case 'float': {
      const want = parseNumber(n.value);
      assert.equal(typeof got, 'number', at);
      assert.ok(Object.is(got, want) || (Number.isNaN(got) && Number.isNaN(want)), `${at}: ${got} != ${want}`);
      return;
    }
    case 'bool':
    case 'str':
      assert.equal(got, n.value, at);
      return;
    case 'datetime':
    case 'date_ms':
      if (MS.test(n.value)) return;
      assert.ok(got instanceof Date, `${at}: ${JSON.stringify(got)}`);
      assert.equal(got.getTime(), Date.parse(n.value), at);
      return;
    default:
      return; // no host type: the term comparison covers it
  }
}

async function freshDb() {
  const { file } = tempDb('case.glv');
  return { file, db: await Database.open(file, OPTS) };
}

async function runOps(ctx, ops, names) {
  for (const [i, op] of ops.entries()) {
    const at = `op ${i} ${op.op}`;
    if (op.op === 'reopen') {
      await ctx.db.close();
      ctx.db = await Database.open(ctx.file, OPTS);
      continue;
    }
    const o = { op: op.op.replace(/_([a-z])/g, (_, c) => c.toUpperCase()) };
    for (const k of ['s', 'p', 'o', 'expected', 'graph', 'child', 'parent', 'type', 'role', 'filler']) {
      if (op[k] !== undefined) o[k] = hostTerm(op[k], names);
    }
    if (op.id !== undefined) o.id = names.get(op.id);
    if (op.mode !== undefined) o.mode = op.mode === 'keep_annotations' ? 'keep' : op.mode;
    for (const k of ['recursive', 'multi', 'functional']) if (op[k] !== undefined) o[k] = op[k];
    if (op.op === 'create_graph') o.name = hostTerm(op.graph ?? op.name, names);
    if (op.op === 'declare_hedge_type') {
      o.roles = op.roles.map((r) => ({ role: hostTerm(r.role, names), many: r.many, required: r.required }));
      if (op.attrs) o.attrs = op.attrs.map((a) => hostTerm(a, names));
    }
    if (op.op === 'create_hedge') {
      o.roles = op.roles.map(([r, f]) => [hostTerm(r, names), hostTerm(f, names)]);
      if (op.attrs) o.attrs = op.attrs.map(([r, f]) => [hostTerm(r, names), hostTerm(f, names)]);
    }
    let res;
    try {
      res = (await ctx.db.submit([o], 'ordered')).results[0];
    } catch (e) {
      assert.ok(e instanceof GraphlavaError, `${at}: ${e}`);
      assert.ok(op.error, `${at}: unexpected ${e.code}: ${e.message}`);
      assert.equal(e.code, code(op.error), `${at}: ${e.message}`);
      continue;
    }
    assert.ok(!op.error, `${at}: expected ${op.error}, got ${String(res)}`);
    if (op.as !== undefined) {
      assert.equal(typeof res, 'bigint', `${at}: no ID to bind`);
      names.set(op.as, res);
    }
    if (op.result !== undefined) {
      if (typeof op.result === 'boolean') assert.equal(res, op.result, at);
      else assert.equal(Array.isArray(res) ? res.length : res, op.result, at);
    }
  }
}

async function roundtrip(c) {
  const host = c.value.native ? hostNative(c.value.native) : SKIP;
  let input;
  try {
    input = host === SKIP ? hostTerm(c.value.term, new Map()) : host;
  } catch (e) {
    return `skipped: ${e.message}`;
  }
  const ctx = await freshDb();
  await ctx.db.submit([{ op: 'insert', s: 'ex:s', p: 'ex:p', o: input }], 'ordered');
  if (c.reopen) {
    await ctx.db.close();
    ctx.db = await Database.open(ctx.file, OPTS);
  }
  const s = await ctx.db.lookup({ iri: `${EX}s` });
  const p = await ctx.db.lookup({ iri: `${EX}p` });
  const ms = await ctx.db.out(s, p);
  assert.equal(ms.length, 1, 'one statement');
  const exp = c.expect || c.value;
  const term = await ctx.db.decodeTerm(ms[0].o);
  const wantTerm = exp.term || c.value.term;
  const norm = (t) => ('literal' in t && t.datatype === undefined && t.lang === undefined ? { ...t, datatype: 'http://www.w3.org/2001/XMLSchema#string' } : t);
  const strip = (t) => {
    if (t.lang !== undefined) return { literal: t.literal, lang: t.lang };
    return t;
  };
  assert.deepEqual(strip(term), strip(norm(wantTerm)), 'decoded term');
  if (exp.native) checkNative(await ctx.db.decode(ms[0].o), exp.native, 'host value');
  if (c.inline !== undefined) assert.equal(ms[0].o >> 63n === 1n, c.inline, 'inline bit');
  await ctx.db.close();
}

async function reject(c) {
  const host = hostNative(c.value.native);
  if (host === SKIP) return `skipped: no Node type ${c.value.native.type}`;
  const ctx = await freshDb();
  await assert.rejects(ctx.db.submit([{ op: 'insert', s: 'ex:s', p: 'ex:p', o: host }], 'ordered'), (e) => {
    assert.ok(e instanceof GraphlavaError);
    assert.equal(e.code, code(c.error));
    return true;
  });
  assert.deepEqual(await ctx.db.find(), []);
  await ctx.db.close();
}

const key = (row) => JSON.stringify(row);

async function query(c) {
  const ctx = await freshDb();
  await runOps(ctx, c.setup || [], new Map());
  const rows = c.lang === 'sparql' ? await ctx.db.sparql(c.text) : await ctx.db.datalog(c.text);
  const e = c.expect;
  if (e.count !== undefined) {
    assert.equal(rows.length, e.count);
    await rows.close();
  } else if (e.boolean !== undefined) {
    const all = await rows.toArray();
    assert.deepEqual(all, [{ ask: e.boolean }]);
  } else {
    assert.deepEqual(rows.vars, e.vars);
    const got = [];
    for await (const b of rows.batches()) {
      const cols = [];
      for (const v of rows.vars) cols.push(await b.terms(v));
      for (let i = 0; i < b.length; i++) got.push(cols.map((col) => col[i]));
    }
    const norm = (t) => {
      if (t === null) return null;
      if ('literal' in t) {
        if (t.lang !== undefined) return { literal: t.literal, lang: t.lang };
        return { literal: t.literal, datatype: t.datatype ?? 'http://www.w3.org/2001/XMLSchema#string' };
      }
      return t;
    };
    const want = e.rows.map((r) => r.map(norm));
    const have = got.map((r) => r.map(norm));
    if (e.ordered) assert.deepEqual(have, want);
    else assert.deepEqual(have.map(key).sort(), want.map(key).sort());
  }
  await ctx.db.close();
}

async function ops(c) {
  const ctx = await freshDb();
  await runOps(ctx, c.ops, new Map());
  await ctx.db.close();
}

// @tg: verifies:: [[openspec:node-binding#Node literal mapping#Integers across the safe and inline limits]] {level: conformance}
// @tg: verifies:: [[openspec:node-binding#Node literal mapping#Unsafe number is rejected]] {level: conformance}
test(`conformance corpus (${cases.length} cases for node)`, async (t) => {
  assert.ok(cases.some((c) => c.id.startsWith('node-')), 'node.jsonl is part of the corpus');
  for (const c of cases) {
    await t.test(c.id, async (st) => {
      const run = { roundtrip, reject, query, ops }[c.kind];
      const note = await run(c);
      if (typeof note === 'string') st.diagnostic(note);
    });
  }
});
