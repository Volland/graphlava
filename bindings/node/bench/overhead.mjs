// Binding overhead for Node ([[17-benchmarks#Binding overhead]]): the time an
// awaited call adds above the same Rust call on the same file, plus the
// event-loop delay (`monitorEventLoopDelay` p99) while 200 `sync` commits run.
//
//   node bench/overhead.mjs [--statements 1000000] [--iterations 20000] [--out file.json]
//
// Local runs are for development only; the baseline comes from the fixed
// Linux x86_64 nightly runner (bench/baselines/binding-overhead-node.json).
import fs from 'node:fs';
import { monitorEventLoopDelay } from 'node:perf_hooks';
import { createRequire } from 'node:module';
import { Database, iri } from '../index.mjs';
import { args, pct, rng, load, environment, tempFile } from './common.mjs';

const require = createRequire(import.meta.url);
const { kNative } = require('../lib/database.js');
const opt = args({ statements: 100000, iterations: 5000, out: '' });

// @tg: verifies:: [[openspec:node-binding#Node call overhead#Overhead benchmark runs nightly]] {level: benchmark}
async function main() {
  const file = tempFile('overhead.glv');
  const db = await Database.open(file, { prefixes: { '': 'http://ex.org/' } });
  const nodes = await load(db, opt.statements);
  const r = rng(7);
  const ids = [];
  for (let i = 0; i < 256; i++) ids.push(await db.lookup(iri(`:n${Math.floor(r() * nodes)}`)));
  const knows = await db.lookup(iri(':knows'));
  const ns = (t0) => Number(process.hrtime.bigint() - t0);
  const res = { environment: environment(), statements: opt.statements, iterations: opt.iterations, unit: 'us', workloads: {} };

  // props(): Node (awaited) vs Rust (in-process, same snapshot work per call).
  const nodeProps = [];
  for (let i = 0; i < opt.iterations; i++) {
    const t0 = process.hrtime.bigint();
    await db.props(ids[i & 255]);
    nodeProps.push(ns(t0) / 1000);
  }
  const rustProps = Array.from(db[kNative].benchRustProps(ids[0], opt.iterations), (x) => x / 1000);
  const n = pct(nodeProps);
  const rs = pct(rustProps);
  res.workloads.props = {
    node: n,
    rust: rs,
    overhead: Object.fromEntries(Object.keys(n).map((k) => [k, n[k] - rs[k]])),
    samplesOverhead: nodeProps.map((x) => x - rs.p50),
  };

  // out() 2-hop traversal.
  const hop = [];
  for (let i = 0; i < Math.min(opt.iterations, 2000); i++) {
    const t0 = process.hrtime.bigint();
    const first = await db.out(ids[i & 255], knows);
    for (const m of first.slice(0, 8)) await db.out(m.o, knows);
    hop.push(ns(t0) / 1000);
  }
  res.workloads.out2hop = { node: pct(hop) };

  // 1-statement ordered submit.
  const sub = [];
  for (let i = 0; i < Math.min(opt.iterations, 2000); i++) {
    const t0 = process.hrtime.bigint();
    await db.submit([{ op: 'insert', s: `:w${i}`, p: ':p', o: i }], 'ordered');
    sub.push(ns(t0) / 1000);
  }
  res.workloads.submitOrdered = { node: pct(sub) };

  // 10,000-row SELECT: row by row and in batches (µs per row).
  const q = 'SELECT ?s ?o WHERE { ?s :knows ?o } LIMIT 10000';
  let t0 = process.hrtime.bigint();
  let rows = 0;
  for await (const _ of await db.sparql(q)) rows++;
  const rowPath = ns(t0) / 1000 / Math.max(rows, 1);
  t0 = process.hrtime.bigint();
  rows = 0;
  for await (const b of (await db.sparql(q)).batches()) rows += b.ids('s').length;
  const batchPath = ns(t0) / 1000 / Math.max(rows, 1);
  res.workloads.select10k = { rows, usPerRowRows: rowPath, usPerRowBatches: batchPath };

  // Event-loop delay while 200 sync commits run back to back.
  const h = monitorEventLoopDelay({ resolution: 1 });
  h.enable();
  for (let i = 0; i < 200; i++) await db.submit([{ op: 'insert', s: `:e${i}`, p: ':p', o: i }], 'sync');
  h.disable();
  res.eventLoopDelayMs = { p50: h.percentile(50) / 1e6, p99: h.percentile(99) / 1e6, max: h.max / 1e6, passLineP99: 20 };

  await db.close();
  const brief = { ...res, workloads: { ...res.workloads, props: { ...res.workloads.props, samplesOverhead: undefined } } };
  console.log(JSON.stringify(brief, null, 2));
  if (opt.out) fs.writeFileSync(opt.out, JSON.stringify(res));
}

await main();
