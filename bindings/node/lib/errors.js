'use strict';
// `GraphlavaError` with a stable `code` ([[16-api#Node.js binding]]).
// @tg: implements:: [[openspec:node-binding#Node errors]]

const MARK = '\u001fGLV\u001f';

/**
 * Every error the package raises. `code` is stable: one per Rust `Error`
 * variant (`LOCKED`, `CONFLICT`, `WRITE_TIMEOUT`, `NOT_FOUND`, `GRAPH_CYCLE`,
 * `GRAPH_DEPTH_EXCEEDED`, `FILLS_ROLE`, `CORRUPTED`, `INVALID`, ...), plus
 * `BUSY`, `CLOSED` and `INTERNAL` (a caught Rust panic). ID fields of a
 * variant (`id`, `item`, `by`, `hedge`, `child`, `parent`) are `bigint`;
 * counts (`limit`) are `number`.
 */
class GraphlavaError extends Error {
  constructor(code, message, fields) {
    super(message);
    this.name = 'GraphlavaError';
    this.code = code;
    if (fields) Object.assign(this, fields);
  }
}

function invalid(message) {
  return new GraphlavaError('INVALID', message);
}

/** Converts an error from the addon into a `GraphlavaError`. */
function fromNative(e) {
  if (e instanceof GraphlavaError) return e;
  const msg = e && typeof e.message === 'string' ? e.message : String(e);
  if (msg.startsWith(MARK)) {
    const rest = msg.slice(MARK.length);
    const a = rest.indexOf('\u001f');
    const b = rest.indexOf('\u001f', a + 1);
    const code = rest.slice(0, a);
    const fields = {};
    for (const kv of rest.slice(a + 1, b).split(';')) {
      if (!kv) continue;
      const eq = kv.indexOf('=');
      const k = kv.slice(0, eq);
      const v = kv.slice(eq + 1);
      fields[k] = v[0] === 'n' ? BigInt(v.slice(1)) : Number(v.slice(1));
    }
    return new GraphlavaError(code, rest.slice(b + 1), fields);
  }
  // napi argument conversion failures: a wrong value type reached the addon.
  if (e && (e.code === 'InvalidArg' || e.code === 'NumberExpected' || e.code === 'StringExpected' ||
      e.code === 'ObjectExpected' || e.code === 'BigintExpected' || e.code === 'ArrayExpected')) {
    return new GraphlavaError('INVALID', msg);
  }
  const err = new GraphlavaError('INTERNAL', msg);
  err.cause = e;
  return err;
}

/** Awaits `f()` and rethrows any failure as a `GraphlavaError`. */
async function call(f) {
  try {
    return await f();
  } catch (e) {
    throw fromNative(e);
  }
}

/** Runs `f()` synchronously, rethrowing as a `GraphlavaError`. */
function callSync(f) {
  try {
    return f();
  } catch (e) {
    throw fromNative(e);
  }
}

module.exports = { GraphlavaError, invalid, fromNative, call, callSync };
