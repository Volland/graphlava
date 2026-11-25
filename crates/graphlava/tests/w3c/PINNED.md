# Pinned W3C conformance target

The conformance runner (`tests/w3c.rs`) checks graphlava against these
fixed versions ([[11-sparql#Conformance]], [ADR-0038](../../../../docs/adr/0038-sparql-surface-choices.md)).
Moving to a newer draft or test commit is an explicit, reviewed change: update
this file, `COMMIT=` in `fetch.sh`, and `skip.txt` together.

## Specification snapshot

| Spec | Snapshot | URL |
| --- | --- | --- |
| SPARQL 1.2 Query Language | W3C Working Draft, 25 June 2026 | <https://www.w3.org/TR/2026/WD-sparql12-query-20260625/> |
| SPARQL 1.1 Query / Update | W3C Recommendation, 21 March 2013 | <https://www.w3.org/TR/sparql11-query/>, <https://www.w3.org/TR/sparql11-update/> |

Newer drafts exist (SPARQL 1.2 Query and Update WD of 19 November 2026, RDF 1.2
Concepts CRD of 16 November 2026); they are not adopted yet.

## Test suites

- Repository: <https://github.com/w3c/rdf-tests>
- Commit: `3cbb83bd6d325bcddb1e2496037adb770a9cd769` (2026-11-23, "Skip
  manifest-all.ttl files during rake-based HTML transcoding. (#430)")
- Download: `crates/graphlava/tests/w3c/fetch.sh` unpacks the tarball of that
  commit into `target/w3c-tests/` (not vendored).

Suites run, in standard mode (`QueryOptions::standard()`: `GRAPH` is direct,
the default graph is the file's default graph):

| Suite | Manifest | Test types |
| --- | --- | --- |
| `sparql11-query` | `sparql/sparql11/manifest-sparql11-query.ttl` | QueryEvaluationTest, Positive/NegativeSyntaxTest11 |
| `sparql11-update` | `sparql/sparql11/manifest-sparql11-update.ttl` | UpdateEvaluationTest, Positive/NegativeUpdateSyntaxTest11 |
| `sparql12` | `sparql/sparql12/manifest.ttl` | QueryEvaluationTest, UpdateEvaluationTest, Positive/NegativeSyntaxTest |

Not run: SPARQL 1.0 (`sparql10/`), federation (`service`, `syntax-fed`),
entailment, protocol, graph store protocol, service description and result
format (CSV/TSV/JSON serialisation) suites.

## Comparison rules

- Solutions are compared as multisets (as sequences when the query's outer
  solution sequence has `ORDER BY`), up to blank-node renaming.
- Numeric and boolean literals compare by value, because storage
  canonicalises them (`"01"^^xsd:integer` reads back as `"1"`); every other
  literal compares by lexical form, datatype and language tag (case-insensitive).
- `CONSTRUCT` results and the dataset after an update compare by graph
  isomorphism; empty graphs are not compared.

## Skip list

`skip.txt` lists every test that fails, one IRI per line with a reason. The
runner fails on a failing test not in the list and on a listed test that
passes, so the list stays exact.

Current counts: 25 skips, 4 in `sparql11-query` and 21 in `sparql12`
(9 triple-term tests that need distinct reifiers per triple or the RDF 1.2
reifier/triple-term split, 7 base direction, 5 parser limits or canonicalised
numerics); none in `sparql11-update`.

Triple terms compare structurally, blank nodes inside them up to renaming.
