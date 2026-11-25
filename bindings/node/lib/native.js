'use strict';
// Loads the prebuilt N-API 9 addon for this platform ([[16-api#Node.js binding]]).
// @tg: implements:: [[openspec:node-binding#Node packaging]]

const fs = require('node:fs');
const path = require('node:path');

/** The six prebuilt targets: `<platform>-<arch>[-<abi>]` → Rust target triple. */
const TARGETS = Object.freeze({
  'linux-x64-gnu': 'x86_64-unknown-linux-gnu',
  'linux-arm64-gnu': 'aarch64-unknown-linux-gnu',
  'linux-x64-musl': 'x86_64-unknown-linux-musl',
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'win32-x64-msvc': 'x86_64-pc-windows-msvc',
});

function detectLibc() {
  if (process.platform !== 'linux') return null;
  try {
    const report = process.report && process.report.getReport();
    const header = report && (typeof report === 'string' ? JSON.parse(report).header : report.header);
    if (header && header.glibcVersionRuntime) return 'gnu';
  } catch {
    // fall through to the file check
  }
  try {
    if (fs.readFileSync('/usr/bin/ldd', 'utf8').includes('musl')) return 'musl';
  } catch {
    // no ldd: assume glibc
  }
  return 'gnu';
}

/** The target name of a host, e.g. `linux-x64-gnu` or `freebsd-x64`. */
function targetName({ platform, arch, libc }) {
  if (platform === 'linux') return `${platform}-${arch}-${libc || 'gnu'}`;
  if (platform === 'win32') return `${platform}-${arch}-msvc`;
  return `${platform}-${arch}`;
}

/**
 * The supported target for a host, or an error naming the host and listing
 * the supported targets.
 */
function resolveTarget(host = { platform: process.platform, arch: process.arch, libc: detectLibc() }) {
  const name = targetName(host);
  if (Object.prototype.hasOwnProperty.call(TARGETS, name)) return name;
  const err = new Error(
    `graphlava: no prebuilt binary for ${host.platform}-${host.arch}` +
      (host.platform === 'linux' ? ` (${name})` : '') +
      `. Supported targets: ${Object.keys(TARGETS).join(', ')}.`,
  );
  err.code = 'UNSUPPORTED_PLATFORM';
  err.platform = name;
  throw err;
}

function load() {
  if (process.env.GRAPHLAVA_NATIVE) return require(process.env.GRAPHLAVA_NATIVE);
  const target = resolveTarget();
  const local = path.join(__dirname, '..', `graphlava.${target}.node`);
  if (fs.existsSync(local)) return require(local);
  try {
    return require(`@graphlava/node-${target}`);
  } catch (e) {
    const err = new Error(
      `graphlava: the binary package @graphlava/node-${target} is not installed (${e.message}). ` +
        'Reinstall without --omit=optional / --no-optional.',
    );
    err.code = 'BINARY_MISSING';
    throw err;
  }
}

module.exports = { TARGETS, resolveTarget, targetName, load };
