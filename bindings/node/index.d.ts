// Type declarations of the `@graphlava/node` package ([[16-api#Node.js binding]]).
// Every method that can do I/O, run a query or wait on a commit returns a
// Promise; blocking variants are declared in `@graphlava/node/sync`.

/** A 64-bit graphlava ID: always a `bigint`, never a `number`. */
export type Id = bigint;

/** Durability of a write: `sync` and `ordered` survive any crash once acknowledged. */
export type Durability = 'sync' | 'ordered' | 'relaxed';

/** An IRI term. Inputs may use a declared prefix (`ex:alice`, `:alice`) or `<iri>`. */
export interface IriTerm {
  iri: string;
}
/** A blank-node term. */
export interface BlankTerm {
  bnode: string;
}
/** An existing item by ID, in a value position. */
export interface IdTerm {
  id: Id;
}
/** A literal with a datatype IRI (default xsd:string). */
export interface TypedLiteral {
  value: string;
  datatype?: string;
}
/** A language-tagged string. */
export interface LangLiteral {
  value: string;
  lang: string;
}
/** A triple term (output only). */
export interface TripleTermValue {
  triple: { s: Value; p: Value; o: Value };
}

/**
 * A term in a value position: `string` → xsd:string, `boolean` → xsd:boolean,
 * `bigint` → xsd:integer, `number` → xsd:integer when it is a safe integer and
 * xsd:double otherwise (NaN and unsafe integers are rejected), `Date` →
 * xsd:dateTime; anything else as a tagged object.
 */
export type ValueInput = string | boolean | bigint | number | Date | IriTerm | BlankTerm | IdTerm | TypedLiteral | LangLiteral;

/** A term in a node position: a string is an IRI, a bigint an ID. */
export type NodeInput = string | Id | IriTerm | BlankTerm | IdTerm;

/**
 * A decoded value: xsd:integer is a `number` inside ±(2^53−1) and a `bigint`
 * outside it; xsd:double a `number`; xsd:dateTime a `Date` when it has a
 * timezone and at most millisecond precision; other terms are tagged objects.
 */
export type Value = string | boolean | number | bigint | Date | IriTerm | BlankTerm | TypedLiteral | LangLiteral | TripleTermValue;

/** A term in the lossless corpus form returned by `decodeTerm` and `Batch.terms`. */
export type RawTerm =
  | { iri: string }
  | { bnode: string }
  | { literal: string; datatype: string; lang?: string }
  | { triple: [RawTerm, RawTerm, RawTerm] };

export type DeleteMode = 'cascade' | 'keep' | 'detach';

/** Every core operation, as a plain object tagged by `op` ([[16-api#Core API]]). */
export type CoreOp =
  | { op: 'insert'; s: NodeInput; p: NodeInput; o: ValueInput; graph?: NodeInput }
  | { op: 'set'; s: NodeInput; p: NodeInput; o: ValueInput }
  | { op: 'update'; id: Id; o: ValueInput; expected?: ValueInput }
  | { op: 'annotate'; id: Id; p: NodeInput; o: ValueInput }
  | { op: 'delete'; id: Id; mode?: DeleteMode }
  | { op: 'remove'; s: NodeInput; p: NodeInput; o: ValueInput; mode?: DeleteMode; graph?: NodeInput }
  | { op: 'statementId'; s: NodeInput; p: NodeInput; o: ValueInput }
  | { op: 'insertUnasserted'; s: NodeInput; p: NodeInput; o: ValueInput }
  | { op: 'declarePredicate'; p: NodeInput; multi?: boolean; functional?: boolean }
  | { op: 'createGraph'; name: NodeInput }
  | { op: 'addMember'; item: Id; graph: NodeInput }
  | { op: 'removeMember'; item: Id; graph: NodeInput }
  | { op: 'moveMember'; item: Id; from: NodeInput; to: NodeInput }
  | { op: 'membershipId'; item: Id; graph: NodeInput }
  | { op: 'nest'; child: NodeInput; parent: NodeInput }
  | { op: 'unnest'; child: NodeInput; parent: NodeInput }
  | { op: 'dropGraph'; graph: NodeInput; recursive?: boolean }
  | {
      op: 'declareHedgeType';
      type: NodeInput;
      roles: Array<NodeInput | { role: NodeInput; many?: boolean; required?: boolean }>;
      attrs?: NodeInput[];
    }
  | {
      op: 'createHedge';
      type: NodeInput;
      graph?: NodeInput;
      roles: Record<string, ValueInput> | Array<[NodeInput, ValueInput]>;
      attrs?: Record<string, ValueInput> | Array<[NodeInput, ValueInput]>;
    }
  | { op: 'updateRole'; hedge: Id; role: NodeInput; filler: ValueInput }
  | { op: 'roleId'; hedge: Id; role: NodeInput; node: ValueInput };

/**
 * The result of one operation: an ID (`null` for an unpromoted inline
 * property), the deleted IDs, whether something changed, or `undefined`.
 */
export type OpResult = Id | null | Id[] | boolean | undefined;

/** Returned once the request's durability level holds. */
export interface CommitTicket {
  results: OpResult[];
  /** Sequence number of the batch the request committed in. */
  batch: bigint;
}

/** A statement or inline property matched by a core read. */
export interface Match {
  s: Id;
  p: Id;
  o: Id;
  /** The statement ID; `null` for an unpromoted inline property. */
  id: Id | null;
}

export interface Triple {
  s: Id;
  p: Id;
  o: Id;
}

export interface Hyperedge {
  id: Id;
  type: Id;
  graph: Id;
  /** `[role, filler]` pairs. */
  roles: Array<[Id, Id]>;
}

export interface UpdateReport {
  inserted: number;
  deleted: number;
  updatedInPlace: number;
}

export interface SweepReport {
  marked: number;
  examined: number;
  deleted: number;
  keptYoung: number;
  keptRereferenced: number;
}

export interface OpenOptions {
  /** IRI prefixes for node strings and SPARQL text, e.g. `{ '': 'http://ex.org/' }`. */
  prefixes?: Record<string, string>;
  cacheBytes?: number;
  /** Group-commit window in milliseconds (default 2). */
  groupWindowMs?: number;
  groupBytes?: number;
  /** Interactive writer time limit in milliseconds (default 100). */
  writeTimeoutMs?: number;
  cascadeLimit?: number;
  graphDepthLimit?: number;
  maxRelaxedRun?: number;
}

/** A row: one value per projected variable; `undefined` when unbound. */
export type Row = Record<string, Value | undefined>;

/** Core reads, on a database (fresh snapshot each), a snapshot or a write transaction. */
export interface CoreReads {
  props(node: Id): Promise<Match[]>;
  out(s: Id, p?: Id | null): Promise<Match[]>;
  inc(o: Id, p?: Id | null): Promise<Match[]>;
  find(s?: Id | null, p?: Id | null, o?: Id | null): Promise<Match[]>;
  stmt(id: Id): Promise<Triple | null>;
  isUnasserted(id: Id): Promise<boolean>;
  hedge(id: Id): Promise<Hyperedge | null>;
  /** The ID of a term, without interning it. */
  lookup(term: ValueInput): Promise<Id | null>;
  /** The graph named by an IRI, without creating it. */
  graph(name: NodeInput): Promise<Id | null>;
  decode(id: Id): Promise<Value | undefined>;
  decodeTerm(id: Id): Promise<RawTerm | null>;
  decodeMany(ids: Iterable<Id>): Promise<Array<Value | undefined>>;
  homeGraph(id: Id): Promise<Id>;
  graphsOf(id: Id): Promise<Id[]>;
  parents(graph: Id): Promise<Id[]>;
  children(graph: Id): Promise<Id[]>;
  ancestors(graph: Id): Promise<Id[]>;
  descendants(graph: Id): Promise<Id[]>;
  scopeStatements(graph: Id, opts?: { direct?: boolean }): Promise<Id[]>;
  hedgesOfType(type: Id): Promise<Id[]>;
  commitCounter(): Promise<number>;
  /** The first violated cross-table invariant, or `null`. */
  checkInvariants(): Promise<string | null>;
}

/** One batch of at most 1024 rows. */
export declare class Batch {
  private constructor();
  readonly vars: string[];
  readonly length: number;
  /** ID columns by variable (0 = unbound). */
  readonly columns: Record<string, BigUint64Array>;
  ids(v: string): BigUint64Array;
  /** Decodes one column, on request. */
  values(v: string): Promise<Array<Value | undefined>>;
  terms(v: string): Promise<Array<RawTerm | null>>;
}

/** A query result; leaving iteration early ends the read snapshot. */
export declare class Rows implements AsyncIterable<Row> {
  private constructor();
  readonly vars: string[];
  readonly length: number;
  [Symbol.asyncIterator](): AsyncGenerator<Row, void, undefined>;
  /** Batches of up to 1024 rows with `BigUint64Array` ID columns. */
  batches(): AsyncGenerator<Batch, void, undefined>;
  toArray(): Promise<Row[]>;
  close(): Promise<void>;
  [Symbol.asyncDispose](): Promise<void>;
}

/** A read snapshot. */
export declare class ReadTxn {
  private constructor();
  sparql(text: string): Promise<Rows>;
  datalog(program: string): Promise<Rows>;
  close(): Promise<void>;
  [Symbol.asyncDispose](): Promise<void>;
}
export interface ReadTxn extends CoreReads {}

/**
 * An interactive write transaction. It holds the single writer for at most the
 * time limit (default 100 ms) from the moment `write()` resolved, including
 * time spent awaiting other work; then every call rejects with
 * `WRITE_TIMEOUT`. Prefer `db.submit(ops)`. `await using` aborts it on scope
 * exit without `commit()`.
 */
export declare class WriteTxn {
  private constructor();
  readonly active: boolean;
  run(op: CoreOp): Promise<OpResult>;
  insert(s: NodeInput, p: NodeInput, o: ValueInput, graph?: NodeInput): Promise<Id | null>;
  set(s: NodeInput, p: NodeInput, o: ValueInput): Promise<Id | null>;
  update(id: Id, o: ValueInput, expected?: ValueInput): Promise<void>;
  annotate(id: Id, p: NodeInput, o: ValueInput): Promise<Id | null>;
  delete(id: Id, mode?: DeleteMode): Promise<Id[]>;
  sparql(text: string): Promise<Rows>;
  datalog(program: string): Promise<Rows>;
  sparqlUpdate(text: string): Promise<UpdateReport>;
  commit(): Promise<{ batch: bigint }>;
  abort(): Promise<void>;
  [Symbol.asyncDispose](): Promise<void>;
}
export interface WriteTxn extends CoreReads {}

/** Administration; runs on the writer thread. */
export declare class Admin {
  private constructor();
  sweepDue(): Promise<boolean>;
  sweep(opts?: { windowMs?: number }): Promise<SweepReport>;
  buildOrdering(perm: 'spo' | 'pos' | 'osp' | 'pso' | 'sop' | 'ops'): Promise<void>;
  buildWantedOrderings(threshold?: number): Promise<string[]>;
  buildValueIndex(p: NodeInput): Promise<void>;
  fileSize(): Promise<number>;
  planCacheStats(): Promise<{ hits: number; misses: number; invalidations: number }>;
}

/**
 * The owner handle of one database file. A second `open` of the file in this
 * process (also from a worker thread) or in another process rejects with
 * `LOCKED`. Handles cannot be posted to workers.
 */
export declare class Database {
  protected constructor();
  static open(path: string, options?: OpenOptions): Promise<Database>;
  /** In-memory metadata. */
  readonly isOpen: boolean;
  readonly prefixes: Record<string, string>;
  read(): Promise<ReadTxn>;
  write(durability?: Durability): Promise<WriteTxn>;
  submit(ops: CoreOp[], durability?: Durability): Promise<CommitTicket>;
  sparql(text: string): Promise<Rows>;
  datalog(program: string): Promise<Rows>;
  sparqlUpdate(text: string, durability?: Durability): Promise<UpdateReport>;
  datalogUpdate(program: string, durability?: Durability): Promise<Rows>;
  admin(): Admin;
  /** Commits any open batch and closes the file; `BUSY` while rows, snapshots or transactions are open. */
  close(): Promise<void>;
  [Symbol.asyncDispose](): Promise<void>;
}
export interface Database extends CoreReads {}

/** The stable error codes: one per Rust `Error` variant, plus `BUSY`, `CLOSED` and `INTERNAL`. */
export type ErrorCode =
  | 'IO'
  | 'CORRUPTED'
  | 'FULL'
  | 'FORMAT_MISMATCH'
  | 'LOCKED'
  | 'CONFLICT'
  | 'NOT_FOUND'
  | 'REFERENCED_BY_STATEMENT'
  | 'FILLS_ROLE'
  | 'GRAPH_CYCLE'
  | 'GRAPH_DEPTH_EXCEEDED'
  | 'HYPEREDGE_SHAPE_MISMATCH'
  | 'CASCADE_LIMIT_EXCEEDED'
  | 'WRITE_TIMEOUT'
  | 'READ_ONLY'
  | 'INVALID'
  | 'BUSY'
  | 'CLOSED'
  | 'INTERNAL';

export declare class GraphlavaError extends Error {
  readonly code: ErrorCode;
  /** NOT_FOUND */
  readonly id?: Id;
  /** REFERENCED_BY_STATEMENT, FILLS_ROLE */
  readonly item?: Id;
  /** REFERENCED_BY_STATEMENT */
  readonly by?: Id;
  /** FILLS_ROLE */
  readonly hedge?: Id;
  /** GRAPH_CYCLE */
  readonly child?: Id;
  /** GRAPH_CYCLE */
  readonly parent?: Id;
  /** GRAPH_DEPTH_EXCEEDED, CASCADE_LIMIT_EXCEEDED */
  readonly limit?: number;
}

/** The graphlava version the addon was built from. */
export declare function version(): string;
export declare function iri(value: string): IriTerm;
export declare function bnode(label: string): BlankTerm;
export declare function literal(value: string | number | bigint | boolean, datatypeOrLang?: string | { lang: string }): TypedLiteral | LangLiteral;
/** Forces xsd:double, also for an integral number. */
export declare function double(x: number): TypedLiteral;
export declare const XSD: string;

export declare const _debug: {
  /** Read snapshots alive in this process. */
  openSnapshots(): number;
  /** Panics in the addon; rejects with `INTERNAL`. */
  panic(): Promise<never>;
};
