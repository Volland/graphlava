// Shared helpers of the node:test suite.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

export const PKG = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const PREFIXES = { prefixes: { '': 'http://ex.org/' } };

/** A fresh temp dir with a `memory.glv` path in it. */
export function tempDb(name = 'memory.glv') {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'graphlava-node-'));
  return { dir, file: path.join(dir, name) };
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Runs a Node script (ES module text) in a child process; resolves with {code, stdout, stderr}. */
export function runNode(source, { args = [], env = {}, onStdout } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [...args, '--input-type=module', '-e', source], {
      cwd: PKG,
      env: { ...process.env, ...env },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d) => {
      stdout += d;
      if (onStdout) onStdout(stdout, child);
    });
    child.stderr.on('data', (d) => {
      stderr += d;
    });
    child.on('error', reject);
    child.on('close', (code, signal) => resolve({ code, signal, stdout, stderr }));
  });
}

export const INDEX = path.join(PKG, 'index.mjs');
