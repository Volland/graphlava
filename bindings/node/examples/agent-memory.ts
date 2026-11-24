// Agent memory with @graphlava/node: one-shot writes, annotations, queries.
import { Database, GraphlavaError, iri, type CommitTicket, type Id, type Row } from '@graphlava/node';

export async function remember(path: string): Promise<number> {
  await using db = await Database.open(path, { prefixes: { '': 'http://ex.org/' } });
  const t: CommitTicket = await db.submit([{ op: 'insert', s: ':alice', p: ':knows', o: iri(':bob') }], 'ordered');
  const stmt = t.results[0];
  if (typeof stmt !== 'bigint') throw new Error('expected a statement ID');
  const id: Id = stmt;
  await db.submit([{ op: 'annotate', id, p: ':confidence', o: 0.9 }]);

  // Compare-and-set, retried on CONFLICT.
  const age = await db.submit([{ op: 'set', s: ':alice', p: ':age', o: 30 }, { op: 'statementId', s: ':alice', p: ':age', o: 30 }]);
  const ageId = age.results[1] as Id;
  try {
    await db.submit([{ op: 'update', id: ageId, o: 31, expected: 30 }], 'sync');
  } catch (e) {
    if (!(e instanceof GraphlavaError) || e.code !== 'CONFLICT') throw e;
  }

  const rows = await db.sparql('SELECT ?c WHERE { << :alice :knows :bob >> :confidence ?c }');
  let n = 0;
  for await (const row of rows) {
    const r: Row = row;
    if (typeof r.c === 'number') n++;
  }
  for await (const batch of (await db.sparql('SELECT ?s WHERE { ?s ?p ?o }')).batches()) {
    const ids: BigUint64Array = batch.ids('s');
    n += ids.length > 0 ? 0 : 1;
  }
  return n;
}

export async function interactive(db: Database): Promise<void> {
  await using w = await db.write('sync');
  await w.insert(':a', ':p', iri(':b'));
  const props = await w.props((await w.lookup(iri(':a'))) ?? 0n);
  if (props.length > 0) await w.commit();
}

export async function errors(db: Database): Promise<string> {
  try {
    await db.submit([{ op: 'nest', child: ':g1', parent: ':g2' }]);
    return 'ok';
  } catch (e) {
    if (e instanceof GraphlavaError && e.code === 'GRAPH_CYCLE') return `${e.child} ${e.parent}`;
    throw e;
  }
}
