'use strict';
// Core operations as plain objects tagged by `op`, and core reads
// ([[16-api#Core API]], [[16-api#Node.js binding]]).
// @tg: implements:: [[openspec:node-binding#Node API parity]]

const { invalid } = require('./errors');
const { valueTerm, nodeTerm, idArg, fromTerm, rawTerm } = require('./terms');

const N = 'node';
const V = 'value';
const I = 'id';
const B = 'bool';
const M = 'mode';

// field → [kind, required]
const OPS = {
  insert: { s: [N, 1], p: [N, 1], o: [V, 1], graph: [N, 0] },
  set: { s: [N, 1], p: [N, 1], o: [V, 1] },
  update: { id: [I, 1], o: [V, 1], expected: [V, 0] },
  annotate: { id: [I, 1], p: [N, 1], o: [V, 1] },
  delete: { id: [I, 1], mode: [M, 0] },
  remove: { s: [N, 1], p: [N, 1], o: [V, 1], mode: [M, 0], graph: [N, 0] },
  statementId: { s: [N, 1], p: [N, 1], o: [V, 1] },
  insertUnasserted: { s: [N, 1], p: [N, 1], o: [V, 1] },
  declarePredicate: { p: [N, 1], multi: [B, 0], functional: [B, 0] },
  createGraph: { name: [N, 1] },
  addMember: { item: [I, 1], graph: [N, 1] },
  removeMember: { item: [I, 1], graph: [N, 1] },
  moveMember: { item: [I, 1], from: [N, 1], to: [N, 1] },
  membershipId: { item: [I, 1], graph: [N, 1] },
  nest: { child: [N, 1], parent: [N, 1] },
  unnest: { child: [N, 1], parent: [N, 1] },
  dropGraph: { graph: [N, 1], recursive: [B, 0] },
  declareHedgeType: { type: [N, 1] },
  createHedge: { type: [N, 1], graph: [N, 0] },
  updateRole: { hedge: [I, 1], role: [N, 1], filler: [V, 1] },
  roleId: { hedge: [I, 1], role: [N, 1], node: [V, 1] },
};

const MODES = new Set(['cascade', 'keep', 'keepAnnotations', 'detach']);

function pairs(v, what, prefixes) {
  if (v === undefined || v === null) return [];
  const entries = Array.isArray(v) ? v : Object.entries(v);
  return entries.map((e, i) => {
    if (!Array.isArray(e) || e.length !== 2) throw invalid(`${what}[${i}] must be a [key, value] pair`);
    return { key: nodeTerm(e[0], `${what}[${i}] key`, prefixes), value: valueTerm(e[1], `${what}[${i}] value`, prefixes) };
  });
}

/** Normalises one core operation for the addon; throws `INVALID`. */
function normOp(op, prefixes, at) {
  const where = at === undefined ? 'op' : `ops[${at}]`;
  if (op === null || typeof op !== 'object') throw invalid(`${where} must be an object tagged by "op"`);
  const name = op.op;
  const spec = typeof name === 'string' && Object.prototype.hasOwnProperty.call(OPS, name) ? OPS[name] : null;
  if (!spec) throw invalid(`${where}: unknown core operation ${JSON.stringify(name)}; one of ${Object.keys(OPS).join(', ')}`);
  const out = { op: name };
  for (const [field, [kind, required]] of Object.entries(spec)) {
    const v = op[field];
    const what = `${name}.${field}`;
    if (v === undefined) {
      if (required) throw invalid(`${name}: missing field \`${field}\``);
      continue;
    }
    switch (kind) {
      case N:
        out[field] = nodeTerm(v, what, prefixes);
        break;
      case V:
        out[field] = valueTerm(v, what, prefixes);
        break;
      case I:
        out[field] = idArg(v, what);
        break;
      case B:
        if (typeof v !== 'boolean') throw invalid(`${what} must be a boolean`);
        out[field] = v;
        break;
      case M:
        if (!MODES.has(v)) throw invalid(`${what} must be "cascade", "keep" or "detach"`);
        out[field] = v;
        break;
    }
  }
  if (name === 'declareHedgeType') {
    if (!Array.isArray(op.roles)) throw invalid('declareHedgeType: missing field `roles` (an array)');
    out.roleSpecs = op.roles.map((r, i) => {
      if (typeof r === 'string' || typeof r === 'bigint') return { role: nodeTerm(r, `roles[${i}]`, prefixes) };
      if (r === null || typeof r !== 'object') throw invalid(`declareHedgeType.roles[${i}] must be a role or {role, many?, required?}`);
      return {
        role: nodeTerm(r.role, `roles[${i}].role`, prefixes),
        many: r.many === undefined ? undefined : Boolean(r.many),
        required: r.required === undefined ? undefined : Boolean(r.required),
      };
    });
    out.attrDecls = (op.attrs || []).map((a, i) => nodeTerm(a, `attrs[${i}]`, prefixes));
  } else if (name === 'createHedge') {
    if (op.roles === undefined) throw invalid('createHedge: missing field `roles`');
    out.roles = pairs(op.roles, 'createHedge.roles', prefixes);
    out.attrs = pairs(op.attrs, 'createHedge.attrs', prefixes);
  }
  return out;
}

function normOps(ops, prefixes) {
  if (!Array.isArray(ops)) throw invalid('submit() takes an array of core operations');
  return ops.map((op, i) => normOp(op, prefixes, i));
}

const DURABILITY = new Set(['sync', 'ordered', 'relaxed']);
function durability(d) {
  if (d === undefined) return 'ordered';
  if (!DURABILITY.has(d)) throw invalid(`durability must be "sync", "ordered" or "relaxed", got ${JSON.stringify(d)}`);
  return d;
}

// ---- reads ------------------------------------------------------------------------

const optId = (v, what) => (v === undefined || v === null ? undefined : idArg(v, what));

/** Core reads: name → (args → request), with the result mapping. */
const READS = {
  props: (node) => ({ f: 'props', id: idArg(node, 'node') }),
  out: (s, p) => ({ f: 'out', s: idArg(s, 's'), p: optId(p, 'p') }),
  inc: (o, p) => ({ f: 'inc', o: idArg(o, 'o'), p: optId(p, 'p') }),
  find: (s, p, o) => ({ f: 'find', s: optId(s, 's'), p: optId(p, 'p'), o: optId(o, 'o') }),
  stmt: (id) => ({ f: 'stmt', id: idArg(id, 'id') }),
  isUnasserted: (id) => ({ f: 'isUnasserted', id: idArg(id, 'id') }),
  hedge: (id) => ({ f: 'hedge', id: idArg(id, 'id') }),
  lookup: (term, prefixes) => ({ f: 'lookup', term: valueTerm(term, 'term', prefixes) }),
  graph: (name, prefixes) => ({ f: 'graph', term: nodeTerm(name, 'name', prefixes) }),
  decode: (id) => ({ f: 'decode', id: idArg(id, 'id') }),
  decodeTerm: (id) => ({ f: 'decode', id: idArg(id, 'id') }),
  decodeMany: (ids) => ({ f: 'decodeMany', ids: Array.from(ids, (x, i) => idArg(x, `ids[${i}]`)) }),
  homeGraph: (id) => ({ f: 'homeGraph', id: idArg(id, 'id') }),
  graphsOf: (id) => ({ f: 'graphsOf', id: idArg(id, 'id') }),
  parents: (g) => ({ f: 'parents', id: idArg(g, 'graph') }),
  children: (g) => ({ f: 'children', id: idArg(g, 'graph') }),
  ancestors: (g) => ({ f: 'ancestors', id: idArg(g, 'graph') }),
  descendants: (g) => ({ f: 'descendants', id: idArg(g, 'graph') }),
  scopeStatements: (g, opts) => ({ f: 'scopeStatements', id: idArg(g, 'graph'), direct: Boolean(opts && opts.direct) }),
  hedgesOfType: (ty) => ({ f: 'hedgesOfType', id: idArg(ty, 'type') }),
  commitCounter: () => ({ f: 'commitCounter' }),
  checkInvariants: () => ({ f: 'checkInvariants' }),
};

const READ_ARITY = {
  props: 1, out: 2, inc: 2, find: 3, stmt: 1, isUnasserted: 1, hedge: 1, lookup: 1, graph: 1, decode: 1, decodeTerm: 1,
  decodeMany: 1, homeGraph: 1, graphsOf: 1, parents: 1, children: 1, ancestors: 1, descendants: 1,
  scopeStatements: 2, hedgesOfType: 1, commitCounter: 0, checkInvariants: 0,
};

/** Builds the request of read `f`; the prefix map rides after the arguments. */
function readReq(f, args, prefixes) {
  const a = Array.from({ length: READ_ARITY[f] }, (_, i) => args[i]);
  return READS[f](...a, prefixes);
}

function postRead(f, out) {
  if (f === 'decode') return fromTerm(out);
  if (f === 'decodeTerm') return rawTerm(out);
  if (f === 'decodeMany') return out.map(fromTerm);
  return out;
}

/** Adds the core read methods to a class whose instances implement `_read(req)`. */
function defineReads(proto, suffix, run) {
  for (const f of Object.keys(READS)) {
    Object.defineProperty(proto, f + suffix, {
      value: function (...args) {
        return run(this, f, args);
      },
      writable: true,
      configurable: true,
    });
  }
}

module.exports = { normOp, normOps, durability, readReq, postRead, defineReads, READS };
