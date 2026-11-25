'use strict';
// Host values ↔ graphlava terms ([[16-api#Node.js binding]]).
//
// Inputs. In node positions (`s`, `p`, `graph`, `name`, `type`, `role`,
// `child`, `parent`, `from`, `to`) a string is an IRI and a bigint is an ID.
// In value positions (`o`, `expected`, `filler`, `node`, attribute values,
// `lookup`) host values map to XSD literals:
//   string → xsd:string, boolean → xsd:boolean, bigint → xsd:integer,
//   number → xsd:integer when Number.isSafeInteger, else xsd:double
//            (NaN and unsafe integers are rejected: pass a bigint),
//   Date → xsd:dateTime (milliseconds, UTC).
// Other terms are tagged objects: {iri}, {bnode}, {id}, {value, lang},
// {value, datatype}. Outputs use the same tags; xsd:integer comes back as a
// number inside ±(2^53−1) and as a bigint outside it.

const { invalid } = require('./errors');

const XSD = 'http://www.w3.org/2001/XMLSchema#';
const XSD_STRING = `${XSD}string`;
const XSD_BOOLEAN = `${XSD}boolean`;
const XSD_INTEGER = `${XSD}integer`;
const XSD_DOUBLE = `${XSD}double`;
const XSD_FLOAT = `${XSD}float`;
const XSD_DATETIME = `${XSD}dateTime`;
const U64 = 1n << 64n;
const MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER);

/** Builds an IRI term (expanded against the database prefixes). */
function iri(value) {
  return { iri: value };
}
/** Builds a blank-node term. */
function bnode(label) {
  return { bnode: label };
}
/** Builds a literal with a datatype IRI or a `{lang}` tag. */
function literal(value, datatypeOrLang) {
  if (datatypeOrLang && typeof datatypeOrLang === 'object' && 'lang' in datatypeOrLang) {
    return { value: String(value), lang: datatypeOrLang.lang };
  }
  return { value: String(value), datatype: datatypeOrLang || XSD_STRING };
}
/** Forces `xsd:double` for a number (also an integer-valued one). */
function double(x) {
  if (typeof x !== 'number') throw invalid(`double() takes a number, got ${typeof x}`);
  return { value: doubleLexical(x), datatype: XSD_DOUBLE };
}

function doubleLexical(x) {
  if (Number.isNaN(x)) return 'NaN';
  if (x === Infinity) return 'INF';
  if (x === -Infinity) return '-INF';
  if (Object.is(x, -0)) return '-0.0E0';
  return String(x);
}

// @tg: implements:: [[openspec:node-binding#Node ID mapping]]
/** Checks an ID argument: a bigint holding an unsigned 64-bit value. */
function idArg(v, what) {
  if (typeof v === 'bigint') {
    if (v < 0n || v >= U64) throw invalid(`${what} must be an ID in 0n..2n**64n-1n, got ${v}n`);
    return v;
  }
  if (typeof v === 'number') {
    throw invalid(
      `${what} must be a bigint ID, got the number ${v}: IDs use all 64 bits and do not fit a number; ` +
        `pass a bigint such as ${Number.isInteger(v) ? `${v}n` : 'the ID returned by submit()'}`,
    );
  }
  if (v === undefined || v === null) throw invalid(`${what} is missing`);
  throw invalid(`${what} must be a bigint ID, got ${typeof v}`);
}

/** The prefix map of a database: `{"": "http://ex.org/", ex: ...}`. */
function prefixMap(p) {
  const m = new Map();
  if (p) {
    for (const [k, v] of Object.entries(p)) {
      if (typeof v !== 'string') throw invalid(`prefix ${k}: must map to an IRI string`);
      m.set(k, v);
    }
  }
  return m;
}

/** Expands `pfx:local` and `<iri>`; anything else must be an absolute IRI. */
function expandIri(s, what, prefixes) {
  if (s.length > 1 && s[0] === '<' && s[s.length - 1] === '>') return s.slice(1, -1);
  const m = /^([A-Za-z][\w.-]*)?:/.exec(s);
  if (m) {
    const pfx = m[1] || '';
    if (prefixes.has(pfx)) return prefixes.get(pfx) + s.slice(m[0].length);
    if (pfx === '') {
      throw invalid(`${what}: "${s}" uses the prefix ":" but no "" prefix is declared (Database.open(path, {prefixes}))`);
    }
    return s; // an absolute IRI with scheme `pfx`
  }
  throw invalid(`${what}: "${s}" is not an IRI: use an absolute IRI, <iri> or a declared prefix`);
}

function tagged(v, what, prefixes) {
  if (v instanceof Date) return dateTerm(v, what);
  if (typeof v.iri === 'string') return { k: 'iri', v: expandIri(v.iri, what, prefixes) };
  if (typeof v.bnode === 'string') return { k: 'blank', v: v.bnode };
  if ('id' in v) return { k: 'id', id: idArg(v.id, what) };
  if ('value' in v) {
    if (typeof v.lang === 'string') return { k: 'lit', v: String(v.value), lang: v.lang };
    const dt = v.datatype === undefined ? XSD_STRING : expandIri(String(v.datatype), `${what}.datatype`, prefixes);
    return { k: 'lit', v: String(v.value), dt };
  }
  throw invalid(`${what}: unsupported object; use a string, number, bigint, boolean, Date, {iri}, {bnode}, {id} or {value, datatype|lang}`);
}

function dateTerm(d, what) {
  const t = d.getTime();
  if (Number.isNaN(t)) throw invalid(`${what}: invalid Date`);
  // XSD canonical form: no `+` on large years, no trailing zeros in the fraction.
  const lex = d
    .toISOString()
    .replace(/^([+-])0*(\d{4,})/, (_, sign, y) => (sign === '-' ? '-' : '') + y)
    .replace(/\.(\d*?)0+Z$/, (_, f) => (f ? `.${f}Z` : 'Z'));
  return { k: 'lit', v: lex, dt: XSD_DATETIME };
}

// @tg: implements:: [[openspec:node-binding#Node literal mapping]]
/** A term in a value position (any term; host values become literals). */
function valueTerm(v, what, prefixes) {
  switch (typeof v) {
    case 'string':
      return { k: 'lit', v, dt: XSD_STRING };
    case 'boolean':
      return { k: 'lit', v: v ? 'true' : 'false', dt: XSD_BOOLEAN };
    case 'bigint':
      return { k: 'lit', v: v.toString(), dt: XSD_INTEGER };
    case 'number':
      if (Number.isNaN(v)) throw invalid(`${what}: NaN cannot be stored; use null-free data or double(NaN) explicitly`);
      if (Number.isInteger(v)) {
        if (!Number.isSafeInteger(v)) {
          throw invalid(
            `${what}: ${v} is an integer outside ±(2^53−1), so the number may already have lost precision; ` +
              'pass a bigint (e.g. 2n ** 60n), or double(x) for a floating-point value',
          );
        }
        return { k: 'lit', v: String(v === 0 ? 0 : v), dt: XSD_INTEGER };
      }
      return { k: 'lit', v: doubleLexical(v), dt: XSD_DOUBLE };
    case 'object':
      if (v === null) throw invalid(`${what} is null`);
      return tagged(v, what, prefixes);
    case 'undefined':
      throw invalid(`${what} is missing`);
    default:
      throw invalid(`${what}: cannot store a ${typeof v}`);
  }
}

/** A term in a node position: a string is an IRI, a bigint an ID. */
function nodeTerm(v, what, prefixes) {
  switch (typeof v) {
    case 'string':
      return { k: 'iri', v: expandIri(v, what, prefixes) };
    case 'bigint':
      return { k: 'id', id: idArg(v, what) };
    case 'number':
      return { k: 'id', id: idArg(v, what) }; // throws: IDs are bigint
    case 'object':
      if (v === null) throw invalid(`${what} is null`);
      return tagged(v, what, prefixes);
    case 'undefined':
      throw invalid(`${what} is missing`);
    default:
      throw invalid(`${what}: expected an IRI string, a bigint ID or a term object, got ${typeof v}`);
  }
}

const DATETIME_MS = /^-?\d{4,}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{1,3})?(Z|[+-]\d\d:\d\d)$/;

/** A term from the addon (`{k, v, dt?, lang?}`) as a host value. */
function fromTerm(t) {
  if (t === undefined || t === null) return undefined;
  switch (t.k) {
    case 'iri':
      return { iri: t.v };
    case 'blank':
      return { bnode: t.v };
    case 'triple':
      return { triple: { s: fromTerm(t.s), p: fromTerm(t.p), o: fromTerm(t.o) } };
    case 'lit':
      if (t.lang !== undefined) return { value: t.v, lang: t.lang };
      switch (t.dt) {
        case XSD_STRING:
          return t.v;
        case XSD_BOOLEAN:
          return t.v === 'true' || t.v === '1';
        case XSD_INTEGER: {
          const b = BigInt(t.v);
          return b >= -MAX_SAFE && b <= MAX_SAFE ? Number(b) : b;
        }
        case XSD_DOUBLE:
        case XSD_FLOAT:
          if (t.v === 'INF' || t.v === '+INF') return Infinity;
          if (t.v === '-INF') return -Infinity;
          if (t.v === 'NaN') return NaN;
          return Number(t.v);
        case XSD_DATETIME:
          if (DATETIME_MS.test(t.v)) {
            const d = new Date(t.v);
            if (!Number.isNaN(d.getTime())) return d;
          }
          return { value: t.v, datatype: t.dt };
        default:
          return { value: t.v, datatype: t.dt };
      }
    default:
      return { value: t.v, datatype: 'unknown' };
  }
}

/**
 * A term from the addon in the lossless corpus form: `{iri}`, `{bnode}`,
 * `{literal, datatype, lang?}` or `{triple: [s, p, o]}`.
 */
function rawTerm(t) {
  if (t === undefined || t === null) return null;
  switch (t.k) {
    case 'iri':
      return { iri: t.v };
    case 'blank':
      return { bnode: t.v };
    case 'triple':
      return { triple: [rawTerm(t.s), rawTerm(t.p), rawTerm(t.o)] };
    case 'lit':
      return t.lang !== undefined ? { literal: t.v, datatype: t.dt, lang: t.lang } : { literal: t.v, datatype: t.dt };
    default:
      return { literal: t.v, datatype: 'unknown' };
  }
}

module.exports = {
  XSD,
  rawTerm,
  iri,
  bnode,
  literal,
  double,
  idArg,
  prefixMap,
  expandIri,
  valueTerm,
  nodeTerm,
  fromTerm,
};
