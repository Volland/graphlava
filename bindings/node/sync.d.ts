// Type declarations of `@graphlava/node/sync`: BLOCKING variants for scripts and CLIs.
import {
  Database as AsyncDatabase,
  CommitTicket,
  CoreOp,
  Durability,
  Hyperedge,
  Id,
  Match,
  NodeInput,
  OpenOptions,
  RawTerm,
  Row,
  Triple,
  UpdateReport,
  Value,
  ValueInput,
} from './index';

export { GraphlavaError, version, iri, bnode, literal, double, XSD } from './index';
export type * from './index';

/** Core reads that block until done. */
export interface SyncCoreReads {
  props(node: Id): Match[];
  out(s: Id, p?: Id | null): Match[];
  inc(o: Id, p?: Id | null): Match[];
  find(s?: Id | null, p?: Id | null, o?: Id | null): Match[];
  stmt(id: Id): Triple | null;
  isUnasserted(id: Id): boolean;
  hedge(id: Id): Hyperedge | null;
  lookup(term: ValueInput): Id | null;
  graph(name: NodeInput): Id | null;
  decode(id: Id): Value | undefined;
  decodeTerm(id: Id): RawTerm | null;
  decodeMany(ids: Iterable<Id>): Array<Value | undefined>;
  homeGraph(id: Id): Id;
  graphsOf(id: Id): Id[];
  parents(graph: Id): Id[];
  children(graph: Id): Id[];
  ancestors(graph: Id): Id[];
  descendants(graph: Id): Id[];
  scopeStatements(graph: Id, opts?: { direct?: boolean }): Id[];
  hedgesOfType(type: Id): Id[];
  commitCounter(): number;
  checkInvariants(): string | null;
}

export interface SyncBatch {
  readonly vars: string[];
  readonly length: number;
  readonly columns: Record<string, BigUint64Array>;
  values(v: string): Array<Value | undefined>;
}

export declare class SyncRows implements Iterable<Row> {
  private constructor();
  readonly vars: string[];
  readonly length: number;
  [Symbol.iterator](): Generator<Row, void, undefined>;
  batches(): Generator<SyncBatch, void, undefined>;
  toArray(): Row[];
  close(): void;
  [Symbol.dispose](): void;
}

export declare class SyncReadTxn {
  private constructor();
  sparql(text: string): SyncRows;
  datalog(program: string): SyncRows;
  close(): void;
  [Symbol.dispose](): void;
}
export interface SyncReadTxn extends SyncCoreReads {}

/** The asynchronous `Database` plus `*Sync` methods that block the event loop. */
export declare class Database extends AsyncDatabase {
  static open(path: string, options?: OpenOptions): Promise<Database>;
  static openSync(path: string, options?: OpenOptions): Database;
  readSync(): SyncReadTxn;
  submitSync(ops: CoreOp[], durability?: Durability): CommitTicket;
  sparqlSync(text: string): SyncRows;
  datalogSync(program: string): SyncRows;
  sparqlUpdateSync(text: string, durability?: Durability): UpdateReport;
  datalogUpdateSync(program: string, durability?: Durability): SyncRows;
  closeSync(): void;
  [Symbol.dispose](): void;
  propsSync(node: Id): Match[];
  outSync(s: Id, p?: Id | null): Match[];
  incSync(o: Id, p?: Id | null): Match[];
  findSync(s?: Id | null, p?: Id | null, o?: Id | null): Match[];
  stmtSync(id: Id): Triple | null;
  hedgeSync(id: Id): Hyperedge | null;
  lookupSync(term: ValueInput): Id | null;
  decodeSync(id: Id): Value | undefined;
}
