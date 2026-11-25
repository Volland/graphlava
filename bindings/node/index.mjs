// ESM entry point of graphlava; the implementation is the CommonJS one.
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const m = require('./index.js');

export const { Database, ReadTxn, WriteTxn, Rows, Batch, Admin, GraphlavaError, version, iri, bnode, literal, double, XSD, _debug } = m;
export default m;
