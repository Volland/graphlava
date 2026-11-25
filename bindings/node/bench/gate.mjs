// Nightly gate of the Node Binding overhead ([[17-benchmarks#Binding overhead]]):
// fails when the p50 props() overhead exceeds the stored baseline by more than
// 20% at 95% confidence (bootstrap interval of the p50). Without a baseline
// file it prints the value to store and passes (the first nightly run sets it).
//
//   node bench/gate.mjs --baseline bench/baselines/binding-overhead-node.json --run run.json
import fs from 'node:fs';
import { args, rng } from './common.mjs';

const opt = args({ baseline: '', run: '', threshold: 0.2 });
const run = JSON.parse(fs.readFileSync(opt.run, 'utf8'));
const s = Float64Array.from(run.workloads.props.samplesOverhead);
const r = rng(1);
const p50 = (a) => Float64Array.from(a).sort()[Math.floor(a.length / 2)];
const boots = [];
for (let b = 0; b < 1000; b++) {
  const x = new Float64Array(s.length);
  for (let i = 0; i < s.length; i++) x[i] = s[Math.floor(r() * s.length)];
  boots.push(p50(x));
}
boots.sort((a, b) => a - b);
const lo = boots[25];
const mid = p50(s);
if (!fs.existsSync(opt.baseline)) {
  console.log(`no baseline at ${opt.baseline}; store this run's p50 overhead ${mid.toFixed(2)} µs as the baseline`);
  process.exit(0);
}
const base = JSON.parse(fs.readFileSync(opt.baseline, 'utf8')).props.overheadP50Us;
const limit = base * (1 + opt.threshold);
console.log(`p50 overhead ${mid.toFixed(2)} µs (95% CI lower ${lo.toFixed(2)}), baseline ${base} µs, limit ${limit.toFixed(2)} µs`);
if (lo > limit) {
  console.error('Binding overhead regression beyond +20% at 95% confidence');
  process.exit(1);
}
const delay = run.eventLoopDelayMs.p99;
if (delay >= run.eventLoopDelayMs.passLineP99) {
  console.error(`event-loop delay p99 ${delay} ms is not under ${run.eventLoopDelayMs.passLineP99} ms`);
  process.exit(1);
}
