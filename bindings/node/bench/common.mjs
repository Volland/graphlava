// Shared pieces of the Node benchmarks ([[17-benchmarks#Binding overhead]]).
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execSync } from 'node:child_process';

export function args(defaults) {
  const out = { ...defaults };
  const a = process.argv.slice(2);
  for (let i = 0; i < a.length; i += 2) {
    const k = a[i].replace(/^--/, '').replace(/-([a-z])/g, (_, c) => c.toUpperCase());
    out[k] = typeof defaults[k] === 'number' ? Number(a[i + 1]) : a[i + 1];
  }
  return out;
}

/** Percentiles of a sample (sorted copy), in the sample's unit. */
export function pct(sample, ps = [50, 99, 99.9]) {
  const s = Float64Array.from(sample).sort();
  const at = (p) => s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))];
  return Object.fromEntries(ps.map((p) => [`p${p}`, at(p)]));
}

/** A seeded PRNG (mulberry32): the dataset is the same for a given seed. */
export function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Agent-memory-shaped data: per node a few inline properties and `:knows`
 * edges, until `n` statements exist. A JS port of the shape of the bench/
 * generator (seed 42), not byte-identical to it.
 */
export async function load(db, n, seed = 42) {
  const r = rng(seed);
  const nodes = Math.max(1, Math.floor(n / 5));
  let made = 0;
  let ops = [];
  for (let i = 0; made < n; i++) {
    const s = `:n${i % nodes}`;
    const k = made % 5;
    if (k < 2) ops.push({ op: 'insert', s, p: `:prop${k}`, o: Math.floor(r() * 1e6) });
    else ops.push({ op: 'insert', s, p: ':knows', o: { iri: `http://ex.org/n${Math.floor(r() * nodes)}` } });
    made++;
    if (ops.length === 5000) {
      await db.submit(ops, 'ordered');
      ops = [];
    }
  }
  if (ops.length) await db.submit(ops, 'ordered');
  return nodes;
}

export function environment() {
  let commit = 'unknown';
  try {
    commit = execSync('git rev-parse HEAD', { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim();
  } catch {
    // not a checkout
  }
  return {
    node: process.version,
    platform: `${process.platform}-${process.arch}`,
    cpu: os.cpus()[0] && os.cpus()[0].model,
    cpus: os.availableParallelism ? os.availableParallelism() : os.cpus().length,
    commit,
    date: new Date().toISOString(),
  };
}

export function tempFile(name) {
  return path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'graphlava-bench-')), name);
}
