// Refinement replay through Node (L1, [[19-verification#Refinement bridge]]).
//
// `lake exe oracle <seed> <n>` emits the Lean model's trace; this test replays
// every operation through the Node API on a real file, exactly as the Rust
// replay `crates/graphlava/tests/refinement.rs` does through the Rust API, and
// checks each result and the final state against the trace. Because the Rust
// replay checks the same trace, a pass means the Node results equal the Rust
// results operation by operation. α as in the Rust replay: model node n ↔
// <http://oracle/n{n}>, predicate p ↔ <http://oracle/p{p}>, lit n ↔ the inline
// integer n, graph 0 ↔ the default graph, graph n ↔ <http://oracle/g{n}>.
//
// Seeds 1..16 × 2,000 operations (GRAPHLAVA_NODE_REFINEMENT_SEEDS / _OPS).
// Skipped when the oracle is not built and `lake` is not on PATH, unless
// GRAPHLAVA_REQUIRE_REFINEMENT=1.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { Database, GraphlavaError } from '../index.mjs';
import { tempDb, PKG } from '../test-support/helpers.mjs';

const NS = 'http://oracle/';
const FORMAL = path.resolve(PKG, '../../formal');
const BIN = path.join(FORMAL, '.lake/build/bin/oracle');
const DEFAULT_GRAPH = { iri: 'https://graphlava.org/ns#defaultGraph' };
const STATEMENT_TAG = 4n;
const HEDGE_TAG = 6n;

function oracle(seed, n) {
  const args = [String(seed), String(n)];
  try {
    const out = fs.existsSync(BIN)
      ? execFileSync(BIN, args, { maxBuffer: 1 << 28 })
      : execFileSync('lake', ['exe', 'oracle', ...args], { cwd: FORMAL, maxBuffer: 1 << 28 });
    return out.toString().trim().split('\n').map((l) => JSON.parse(l));
  } catch (e) {
    if (e.code === 'ENOENT') return null;
    throw e;
  }
}

const graphRef = (g) => (g === 0 ? DEFAULT_GRAPH : { iri: `${NS}g${g}` });
const blocked = (e) => e.code === 'REFERENCED_BY_STATEMENT' || e.code === 'FILLS_ROLE';
const kind = (id) => id >> 60n;

class Replay {
  constructor(db) {
    this.db = db;
    this.toRust = new Map();
    this.toModel = new Map();
  }
  bind(expect, id, ctx) {
    if (this.toRust.has(expect)) {
      assert.equal(this.toRust.get(expect), id, `${ctx}: existing item`);
    } else {
      assert.ok(!this.toModel.has(id), `${ctx}: Rust returned an existing ID for a fresh model ID`);
      this.toRust.set(expect, id);
      this.toModel.set(id, expect);
    }
  }
  term(v) {
    if ('node' in v) return { iri: `${NS}n${v.node}` };
    if ('lit' in v) return v.lit;
    return { id: this.toRust.get(v.stmt) };
  }
  rustId(model) {
    // An ID the model never allocated maps to an ID Rust never allocates.
    return this.toRust.get(model) ?? ((STATEMENT_TAG << 60n) | (((1n << 40n) - 1n) << 20n) | BigInt(model % (1 << 20)));
  }
  async submit(ops) {
    try {
      return { ok: true, results: (await this.db.submit(ops, 'relaxed')).results };
    } catch (e) {
      assert.ok(e instanceof GraphlavaError, String(e));
      return { ok: false, err: e };
    }
  }
  removed(gone) {
    return [...gone].map((g) => this.toModel.get(g)).sort((a, b) => a - b);
  }
  async step(line, at) {
    const ctx = `op ${at}: ${JSON.stringify(line)}`;
    const e = line.expect;
    const want = () => [...e.removed].sort((a, b) => a - b);
    switch (line.op) {
      case 'insert': {
        const t = line.triple;
        const s = this.term(t.s);
        const o = this.term(t.o);
        const p = { iri: `${NS}p${t.p}` };
        const r = await this.submit([{ op: 'insert', s, p, o, graph: graphRef(line.graph) }]);
        assert.ok(r.ok, `${ctx}: ${r.err}`);
        let id = r.results[0];
        if (id === null) {
          const q = await this.submit([{ op: 'statementId', s, p, o }]);
          assert.ok(q.ok, `${ctx}: promote: ${q.err}`);
          id = q.results[0];
        }
        this.bind(e.id, id, ctx);
        break;
      }
      case 'hedge': {
        const roles = line.roles.map(([role, filler]) => [{ iri: `${NS}r${role}` }, this.term(filler)]);
        const r = await this.submit([{ op: 'createHedge', type: { iri: `${NS}T${line.type}` }, graph: graphRef(line.graph), roles }]);
        if (r.ok && e.id !== undefined) {
          assert.equal(kind(r.results[0]), HEDGE_TAG, ctx);
          this.bind(e.id, r.results[0], ctx);
        } else {
          assert.ok(!r.ok && r.err.code === 'HYPEREDGE_SHAPE_MISMATCH' && e.id === undefined, `${ctx}: got ${r.err ?? r.results}`);
        }
        break;
      }
      case 'add_member':
      case 'remove_member': {
        const item = this.rustId(line.id);
        if (kind(item) === HEDGE_TAG) {
          assert.equal(e.ok, false, ctx);
          return;
        }
        const op = line.op === 'add_member' ? 'addMember' : 'removeMember';
        const r = await this.submit([{ op, item, graph: graphRef(line.graph) }]);
        if (op === 'addMember') {
          assert.ok((r.ok && e.ok) || (!r.ok && !e.ok && r.err.code === 'NOT_FOUND'), `${ctx}: got ${r.err ?? r.results}`);
        } else if (r.ok) {
          assert.ok(e.ok, ctx);
          assert.deepEqual(this.removed(r.results[0]), want(), `${ctx}: removed set`);
        } else {
          assert.ok(!e.ok, `${ctx}: ${r.err}`);
          if (e.blocked) assert.ok(blocked(r.err), `${ctx}: ${r.err.code}`);
          else assert.ok(['NOT_FOUND', 'INVALID'].includes(r.err.code), `${ctx}: ${r.err.code}`);
        }
        break;
      }
      case 'nest':
      case 'unnest': {
        const r = await this.submit([{ op: line.op, child: graphRef(line.child), parent: graphRef(line.parent) }]);
        let ok;
        if (r.ok) ok = line.op === 'nest' ? true : r.results[0] === true;
        else if (line.op === 'nest' && r.err.code === 'GRAPH_CYCLE') ok = false;
        else if (line.op === 'unnest' && r.err.code === 'INVALID') ok = false;
        else assert.fail(`${ctx}: got ${r.err}`);
        assert.equal(ok, e.ok, ctx);
        break;
      }
      case 'scope': {
        const g = line.graph === 0 ? await this.db.graph(DEFAULT_GRAPH) : await this.db.graph({ iri: `${NS}g${line.graph}` });
        const got = g === null ? [] : (await this.db.scopeStatements(g, { direct: line.direct })).map((id) => this.toModel.get(id)).sort((a, b) => a - b);
        assert.deepEqual(got, e.ids, `${ctx}: scope`);
        break;
      }
      case 'update': {
        const r = await this.submit([{ op: 'update', id: this.rustId(line.id), o: this.term(line.o) }]);
        assert.ok((r.ok && e.ok) || (!r.ok && !e.ok && ['CONFLICT', 'NOT_FOUND'].includes(r.err.code)), `${ctx}: got ${r.err ?? 'ok'}`);
        break;
      }
      case 'delete': {
        const r = await this.submit([{ op: 'delete', id: this.rustId(line.id), mode: line.detach ? 'detach' : 'cascade' }]);
        if (r.ok) {
          assert.ok(e.ok, ctx);
          assert.deepEqual(this.removed(r.results[0]), want(), `${ctx}: removed set`);
        } else {
          assert.ok(!e.ok, `${ctx}: ${r.err}`);
          assert.ok(e.blocked ? blocked(r.err) : r.err.code === 'NOT_FOUND', `${ctx}: ${r.err.code}`);
        }
        break;
      }
      default:
        assert.fail(`unknown op ${line.op}`);
    }
  }
  async termJson(id) {
    if (kind(id) === STATEMENT_TAG) return { stmt: this.toModel.get(id) };
    const t = await this.db.decode(id);
    if (typeof t === 'number' || typeof t === 'bigint') return { lit: Number(t) };
    return { node: Number(t.iri.slice(`${NS}n`.length)) };
  }
  async checkFinal(line, seed) {
    const db = this.db;
    assert.equal(await db.checkInvariants(), null, `seed ${seed}: invariants`);
    const got = [];
    for (const m of await db.find()) {
      const p = await db.decode(m.p);
      if (!p.iri || !p.iri.startsWith(`${NS}p`)) continue; // virtual triples are not model statements
      assert.notEqual(m.id, null, 'every replayed statement has an ID');
      got.push([this.toModel.get(m.id), { s: await this.termJson(m.s), p: Number(p.iri.slice(`${NS}p`.length)), o: await this.termJson(m.o) }]);
    }
    got.sort((a, b) => a[0] - b[0]);
    const want = [...line.final].sort((a, b) => a[0] - b[0]);
    assert.deepEqual(got, want, `seed ${seed}: final state differs`);
    const graphNo = async (g) => {
      const t = await db.decode(g);
      return t.iri === DEFAULT_GRAPH.iri ? 0 : Number(t.iri.slice(`${NS}g`.length));
    };
    for (const [mid, gs] of line.members) {
      const id = this.toRust.get(mid);
      const have = [];
      for (const g of await db.graphsOf(id)) have.push(await graphNo(g));
      have.sort((a, b) => a - b);
      assert.deepEqual(have, gs, `seed ${seed}: memberships of ${mid}`);
    }
    for (const [mid, h] of line.hedges) {
      const id = this.toRust.get(mid);
      const he = await db.hedge(id);
      assert.notEqual(he, null, `seed ${seed}: hyperedge ${mid} is gone`);
      assert.equal((await db.decode(he.type)).iri, `${NS}T${h.type}`);
      const roles = [];
      for (const [r, f] of he.roles) roles.push([(await db.decode(r)).iri, await this.termJson(f)]);
      roles.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
      const wantRoles = h.roles.map(([r, f]) => [`${NS}r${r}`, f]).sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
      assert.deepEqual(roles, wantRoles, `seed ${seed}: roles of ${mid}`);
    }
    let live = 0;
    for (const id of this.toRust.values()) if (kind(id) === HEDGE_TAG && (await db.hedge(id)) !== null) live++;
    assert.equal(live, line.hedges.length, `seed ${seed}: live hyperedges`);
    assert.equal(this.toRust.size + 1, line.next, `seed ${seed}: allocator`);
  }
}

const SEEDS = Number(process.env.GRAPHLAVA_NODE_REFINEMENT_SEEDS || 16);
const OPS = Number(process.env.GRAPHLAVA_NODE_REFINEMENT_OPS || 2000);
const available = oracle(1, 1) !== null;
if (!available && process.env.GRAPHLAVA_REQUIRE_REFINEMENT) throw new Error('oracle not found but GRAPHLAVA_REQUIRE_REFINEMENT is set');

// @tg: verifies:: [[openspec:node-binding#Node API parity#Oracle trace replay through Node]] {level: refinement}
test(`oracle traces replay through Node (seeds 1..${SEEDS} × ${OPS})`, { skip: !available && 'oracle not built and lake not on PATH', timeout: 30 * 60 * 1000 }, async () => {
  for (let seed = 1; seed <= SEEDS; seed++) {
    const trace = oracle(seed, OPS);
    const { dir, file } = tempDb('replay.glv');
    let db = await Database.open(file);
    const schema = trace[0].schema;
    for (const p of schema.multi) await db.submit([{ op: 'declarePredicate', p: { iri: `${NS}p${p}` }, multi: true }], 'relaxed');
    for (const t of schema.htypes) {
      const roles = t.roles.map((r) => ({ role: { iri: `${NS}r${r}` }, required: t.required.includes(r) }));
      await db.submit([{ op: 'declareHedgeType', type: { iri: `${NS}T${t.id}` }, roles }], 'relaxed');
    }
    const rp = new Replay(db);
    const body = trace.slice(1, -1);
    for (let i = 0; i < body.length; i++) {
      await rp.step(body[i], i);
      if (i === 1000) {
        // Close and reopen mid-trace, as the Rust replay does.
        await rp.db.close();
        rp.db = await Database.open(file);
      }
    }
    await rp.checkFinal(trace[trace.length - 1], seed);
    await rp.db.close();
    fs.rmSync(dir, { recursive: true, force: true });
    db = null;
  }
});
