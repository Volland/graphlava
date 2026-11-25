// ESM entry point of @graphlava/node/sync (blocking calls, for scripts and CLIs).
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const m = require('./sync.js');

export const { Database, SyncReadTxn, SyncRows, GraphlavaError, version, iri, bnode, literal, double, XSD } = m;
export default m;
