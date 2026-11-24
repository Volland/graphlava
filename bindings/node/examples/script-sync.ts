// A blocking CLI script with @graphlava/node/sync.
import { Database, iri } from '@graphlava/node/sync';

export function count(path: string): number {
  using db = Database.openSync(path, { prefixes: { ex: 'http://ex.org/' } });
  db.submitSync([{ op: 'insert', s: 'ex:a', p: 'ex:p', o: iri('ex:b') }], 'sync');
  using r = db.readSync();
  const a = r.lookup(iri('ex:a'));
  return a === null ? 0 : r.props(a).length + db.sparqlSync('SELECT * WHERE { ?s ?p ?o }').toArray().length;
}
