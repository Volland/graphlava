// Durability commit latency through Node vs Rust ([[17-benchmarks#Durability commit latency]]):
// 1-statement submits at sync / ordered / relaxed from 1, 8 and 64 concurrent
// callers; acknowledgment latency p50/p99 per level and commits per second.
// The Rust side is the same workload through the native submit with no JS
// awaiting in between (graphlava/sync submitSync, one caller).
//
//   node bench/commit-latency.mjs [--commits 400]
import { Database } from '../sync.mjs';
import { args, pct, environment, tempFile } from './common.mjs';

const opt = args({ commits: 400 });
const out = { environment: environment(), unit: 'us', levels: {} };
for (const level of ['sync', 'ordered', 'relaxed']) {
  out.levels[level] = {};
  for (const callers of [1, 8, 64]) {
    const db = await Database.open(tempFile('commit.glv'), { prefixes: { '': 'http://ex.org/' } });
    const lat = [];
    let next = 0;
    const t0 = process.hrtime.bigint();
    await Promise.all(
      Array.from({ length: callers }, async () => {
        while (next < opt.commits) {
          const i = next++;
          const s = process.hrtime.bigint();
          await db.submit([{ op: 'insert', s: `:c${i}`, p: ':p', o: i }], level);
          lat.push(Number(process.hrtime.bigint() - s) / 1000);
        }
      }),
    );
    const secs = Number(process.hrtime.bigint() - t0) / 1e9;
    out.levels[level][`node_x${callers}`] = { ...pct(lat, [50, 99]), commitsPerSec: opt.commits / secs };
    await db.close();
  }
  // Baseline without the binding's async hop: blocking submits, one caller.
  const db = Database.openSync(tempFile('commit-sync.glv'), { prefixes: { '': 'http://ex.org/' } });
  const lat = [];
  for (let i = 0; i < opt.commits; i++) {
    const s = process.hrtime.bigint();
    db.submitSync([{ op: 'insert', s: `:c${i}`, p: ':p', o: i }], level);
    lat.push(Number(process.hrtime.bigint() - s) / 1000);
  }
  out.levels[level].blocking_x1 = pct(lat, [50, 99]);
  out.levels[level].asyncDeltaP50 = out.levels[level].node_x1.p50 - out.levels[level].blocking_x1.p50;
  db.closeSync();
}
console.log(JSON.stringify(out, null, 2));
