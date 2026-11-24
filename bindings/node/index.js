'use strict';
// graphlava for Node.js: the asynchronous API ([[16-api#Node.js binding]]).
// Blocking variants are only in `@graphlava/node/sync`.

const db = require('./lib/database');
const terms = require('./lib/terms');

module.exports = {
  Database: db.Database,
  ReadTxn: db.ReadTxn,
  WriteTxn: db.WriteTxn,
  Rows: db.Rows,
  Batch: db.Batch,
  Admin: db.Admin,
  GraphlavaError: db.GraphlavaError,
  version: db.version,
  iri: terms.iri,
  bnode: terms.bnode,
  literal: terms.literal,
  double: terms.double,
  XSD: terms.XSD,
  _debug: db._debug,
};
