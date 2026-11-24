// `await using` bodies, loaded only on Node versions that parse the syntax.
import { iri } from '../index.mjs';

export async function insertThreeThenThrow(db, err) {
  await using w = await db.write('sync');
  await w.insert(':a', ':p', iri(':x1'));
  await w.insert(':a', ':p', iri(':x2'));
  await w.insert(':a', ':p', iri(':x3'));
  throw err;
}
