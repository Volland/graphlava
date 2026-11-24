// Packaging ([[openspec:node-binding#Node packaging]]).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { resolveTarget, TARGETS } = require('../lib/native.js');

// @tg: verifies:: [[openspec:node-binding#Node packaging#Unsupported platform]] {level: unit}
test('loading on an unsupported platform names it and lists the supported targets', () => {
  assert.throws(
    () => resolveTarget({ platform: 'freebsd', arch: 'x64', libc: null }),
    (e) => {
      assert.match(e.message, /freebsd-x64/);
      for (const t of Object.keys(TARGETS)) assert.ok(e.message.includes(t), t);
      assert.equal(e.code, 'UNSUPPORTED_PLATFORM');
      return true;
    },
  );
  assert.throws(() => resolveTarget({ platform: 'linux', arch: 'arm64', libc: 'musl' }), /linux-arm64-musl/);
  assert.equal(Object.keys(TARGETS).length, 6);
  for (const [platform, arch, libc, name] of [
    ['linux', 'x64', 'gnu', 'linux-x64-gnu'],
    ['linux', 'arm64', 'gnu', 'linux-arm64-gnu'],
    ['linux', 'x64', 'musl', 'linux-x64-musl'],
    ['darwin', 'arm64', null, 'darwin-arm64'],
    ['darwin', 'x64', null, 'darwin-x64'],
    ['win32', 'x64', null, 'win32-x64-msvc'],
  ]) {
    assert.equal(resolveTarget({ platform, arch, libc }), name);
  }
});

test('ESM and CommonJS entry points expose the same API', async () => {
  const cjs = require('../index.js');
  const esm = await import('../index.mjs');
  assert.equal(esm.Database, cjs.Database);
  assert.equal(esm.GraphlavaError, cjs.GraphlavaError);
  const syncCjs = require('../sync.js');
  const syncEsm = await import('../sync.mjs');
  assert.equal(syncEsm.Database, syncCjs.Database);
  assert.ok(syncCjs.Database.prototype instanceof cjs.Database);
  const pkg = require('../package.json');
  assert.equal(Object.keys(pkg.optionalDependencies).length, 6);
  assert.deepEqual(Object.keys(pkg.exports).sort(), ['.', './package.json', './sync']);
});
