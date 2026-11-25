---
type: Requirement
openspec: [datalog-query]
status: draft
priority: must
specs: ["adr:0009", "adr:0013", "adr:0014", "adr:0008", "adr:0005", "adr:0029"]
---
# Datalog

The Datalog surface and engine: Nemo-compatible syntax, standalone queries and SPARQL embedding, stratification, on-demand evaluation with magic sets, and budgeted incremental maintenance of materialised rules.

- follows_from:: [[adr:0009]]
- follows_from:: [[adr:0013]]
- follows_from:: [[adr:0014]]
- depends_on:: [[10-ir#Operators]] {strength: hard}

## Purpose

Datalog is the third query surface and the language of rules: reasoning, graph closure (`in_star`) and user views. Rules compile to the shared IR and run on the same planner as SPARQL.

The engine is built in-house (handover D15). It borrows: the Nemo rule syntax (Ivliev et al., "Nemo: a scalable and versatile Datalog engine", 2024), Datafrog-style sorted-relation join kernels, the parser and stratification code of oxilite where it fits, and Ascent for compile-time built-in rule sets.

<non-goal>

No inventing a new Datalog dialect, no existential rules (Nemo's `!`-variables) in v0, and no Datalog-over-history: graphlava has no time dimension.

</non-goal>

## Surface syntax

Nemo-compatible syntax with an RDF-aware data model. Where Nemo has a construct, graphlava uses the same spelling; extensions are annotations, never new punctuation.

| Construct | Syntax | Notes |
| --- | --- | --- |
| Fact | `edge(:a, :b) .` | constants are IRIs (prefixed or `<…>`), literals or numbers |
| Rule | `reach(?x, ?z) :- reach(?x, ?y), edge(?y, ?z) .` | Nemo variables start with `?` |
| Negation | `~blocked(?x)` | stratified only |
| Built-ins | `?n = ?a + 1`, `?a > 3`, `STRLEN(?s)` | Nemo built-in names where they exist |
| Aggregation | `cnt(?x, #count(?y)) :- edge(?x, ?y) .` | Nemo `#count`, `#sum`, `#min`, `#max` |
| Prefix | `@prefix : <http://example.org/> .` | |
| Store relations | `triple(?s, ?p, ?o)`, `triple(?s, ?p, ?o, ?id)`, `prop`, `role`, `in`, `in_star` | the virtual relations of [[10-ir#Virtual relations]] |
| Predicate as relation | `:knows(?x, ?y)` | shorthand for `triple(?x, :knows, ?y)` |
| Output | `@output reach .` | Nemo output declaration |
| Query | `?- reach(:alice, ?y) .` | graphlava extension for interactive queries |
| Rule annotation | `@materialized`, `@materialized(force)`, `@on_demand` | before a rule or a predicate declaration |

<rule id="dl-nemo-compatible" strength="must">

A Nemo program that uses only facts, rules, stratified negation, aggregation and built-ins that graphlava supports parses and gives the same results in graphlava, apart from data import directives (`@import`), which are replaced by reading the store.

</rule>

<rule id="dl-annotation-scope" strength="must">

`@materialized` and `@on_demand` apply to a predicate, not to one rule: all rules with the same head predicate share one mode. Conflicting annotations on rules of one head are a compile error.

</rule>

<example>

<input>

```
@prefix : <http://example.org/> .
@materialized
ancestor(?x, ?y) :- :parent(?x, ?y) .
ancestor(?x, ?z) :- ancestor(?x, ?y), :parent(?y, ?z) .
?- ancestor(:alice, ?who) .
```

</input>

<output>

`ancestor` is materialised (`:parent` is a stable, node-valued predicate). The query reads the derived table with the first column bound.

</output>

</example>

## Standalone queries

Datalog is a standalone query language in v0, not only reachable through SPARQL. Programs, queries and rule definitions are submitted directly and run on the same IR, planner and transaction API.

- follows_from:: [[adr:0014]]
- depends_on:: [[16-api#Rust API]] {strength: hard}

<rule id="dl-standalone-entry" strength="must">

The Datalog parser has its own entry point in the API ([[16-api#Rust API]]) that accepts a program text. A program may hold facts, rules, `@output` declarations and `?-` queries; no SPARQL wrapper is needed.

</rule>

<rule id="dl-standalone-query" strength="must">

Results come from `?- goal .` queries, or from the predicates named in `@output`. Each result is a stream of ID tuples, decoded to terms only at projection ([[10-ir#Operators]]), exactly as for SPARQL results.

</rule>

<rule id="dl-standalone-rule-def" strength="must">

Rule definitions can be stored: `define` stores a program's rules in the meta-model ([[01-data-model#Meta-model]]) under their head IRIs, after which SPARQL (`SERVICE <meta:datalog>`) and later Datalog programs can use them. A program submitted for one query keeps its rules transient.

</rule>

<rule id="dl-standalone-txn" strength="must">

A Datalog request runs inside a transaction of the same API as SPARQL: a read snapshot for queries, the single writer for anything that changes state, and the caller's durability level ([[06-transactions-durability#Durability levels]]).

</rule>

<rule id="dl-standalone-shared-ir" strength="must">

The standalone path and the `SERVICE <meta:datalog>` path compile through the same parser, the same IR translation and the same planner. A query gives the same result from either path.

</rule>

### Writes from Datalog

Facts in a Datalog program can be asserted into base data, not only into derived relations. How exactly a program marks a base write is an open item.

<rule id="dl-write-base" strength="should">

A fact or rule head whose predicate is a store predicate (`triple(…)` or `:p(…)`) and that is marked as an assertion inserts base statements in a write transaction. Base writes are ordinary statements with IDs, unlike derived facts ([[adr:0009]]); the marking is the `@assert` directive ([[adr:0040]]). Programs accept Nemo `@output` and a `?- goal.` shorthand that compiles to it.

</rule>

<example>

<input>

```
@assert :knows(:alice, :bob) .
```

</input>

<output>

One base statement `(:alice, :knows, :bob)` with a statement ID, in the caller's write graph. Without an assertion marker the same line is a transient fact, visible only to the program.

</output>

</example>

## Stratification

How programs with negation and aggregation are ordered into strata. Only stratified programs are accepted; the rule is checked at compile time.

<rule id="dl-stratified" strength="must">

The predicate dependency graph is split into strongly connected components. A program is rejected if any component contains a negative edge or an aggregation edge. Strata are evaluated bottom-up in topological order. Cost: O(rules + body atoms).

</rule>

<rule id="dl-safety" strength="must">

Every variable in a head, in a negated atom or in a built-in must appear in a positive body atom of the same rule. Unsafe rules are a compile error with the variable name.

</rule>

The oxilite stratifier is the starting point; its output (strata as lists of rule sets) maps one-to-one onto IR `Fixpoint` nodes.

## On-demand evaluation

The default mode. A rule set is evaluated per query, starting from the query's bound arguments through the magic-sets rewrite, and run to a fixpoint with semi-naive evaluation.

- follows_from:: [[adr:0009]]

<rule id="dl-default-on-demand" strength="must">

A rule head with no annotation is `@on_demand`. The planner never maintains it at commit.

</rule>

<rule id="dl-magic-sets" strength="must">

When a query binds arguments of an on-demand predicate, the program is rewritten with magic sets (Bancilhon, Maier, Sagiv, Ullman, "Magic sets and other strange ways to implement logic programs", PODS 1986) using sideways information passing from left to right in the body order chosen by the planner. The rewrite is skipped if no argument is bound.

</rule>

<rule id="dl-semi-naive" strength="must">

Recursive strata use semi-naive evaluation: each iteration joins only the delta of the previous iteration. Delta and full relations are sorted ID-tuple vectors (Datafrog style), so joins are merge joins or Leapfrog Triejoin ([[14-planner-execution#Leapfrog Triejoin]]).

</rule>

<rule id="dl-on-demand-limits" strength="must">

On-demand evaluation runs under the query's row and time budgets. Exceeding a budget aborts the query with an error that names the predicate; it never returns a partial fixpoint as if complete.

</rule>

Complexity: semi-naive evaluation does O(total derivations) work per stratum; magic sets limit derivations to facts relevant to the bound arguments (no general bound; to be measured on the benchmark rule sets in [[17-benchmarks#Datalog versus Nemo]]).

## Materialized maintenance

How `@materialized` predicates are kept current inside the writer transaction: which predicates qualify, counting for non-recursive rules, Backward/Forward with a budget for recursive ones, and the stale fallback.

- follows_from:: [[adr:0009]]
- follows_from:: [[adr:0029]]
- depends_on:: [[06-transactions-durability#Group commit]] {strength: soft}

<rule id="dl-mat-stable-only" strength="must">

`@materialized` is accepted only if every body predicate is stable: a node-valued edge predicate whose measured overwrite rate is under a configured threshold. Inline properties are never stable ([[adr:0005]]). Otherwise declaration fails with the name of the hot predicate, unless the annotation is `@materialized(force)`.

</rule>

<rule id="dl-mat-counting" strength="must">

Non-recursive materialised rules are maintained by counting (Gupta, Mumick, Subrahmanian, "Maintaining views incrementally", SIGMOD 1993): each derived fact stores its derivation count; inserts and deletes of base facts adjust counts by delta joins, and a fact is removed when its count reaches 0. Cost is proportional to the delta joins.

</rule>

<rule id="dl-mat-bf" strength="must">

Recursive materialised rules are maintained by the Backward/Forward algorithm (Motik, Nenov, Piro, Horrocks, "Incremental update of Datalog materialisation: the Backward/Forward algorithm", AAAI 2015). For each derived fact that a deletion may invalidate, a backward search over the remaining facts looks for another derivation before the fact is deleted; only facts without one are deleted and propagated forward. New derivations are inserted semi-naively. Unlike DRed (delete and re-derive), it does not over-delete, which keeps maintenance within the budget of `dl-mat-budget` far more often ([[adr:0029]]).

</rule>

<rule id="dl-mat-budget" strength="must">

Each transaction has a budget on derived facts touched by maintenance. When a rule exceeds it, the rule is marked `stale` in the same transaction, its maintenance stops, and the commit proceeds. Reads of a stale predicate fall back to on-demand evaluation; a background job rebuilds the derived table and clears `stale` in a later transaction.

</rule>

<rule id="dl-mat-derived-table" strength="must">

Derived facts live in a separate derived table, never in the base orderings. They have no statement ID until something annotates them, at which point they get one lazily ([[adr:0009]]).

</rule>

<rule id="dl-mat-in-star" strength="must">

The graph closure `in_star` is a built-in `@materialized` rule over `:in` between graphs. Its DAG shape and depth limit ([[09-graphs#Nesting DAG]]) allow a specialised maintenance: inserts add the product of descendants and ancestors, and deletes recompute ancestor sets of the affected descendants in topological order, so dropping one of two parent links never removes a still-reachable ancestor and no path count can overflow.

</rule>

<example>

<input>

`@materialized` `ancestor` over `:parent`; a transaction deletes `(:bob :parent :carol)`.

</input>

<output>

Backward/Forward checks each `ancestor(x, carol)` reached through `bob` for another derivation, deletes only the facts that have none, and commits. If more than the budget of facts is touched, `ancestor` becomes stale and queries use on-demand evaluation until the rebuild ends.

</output>

</example>

### Built-in rule sets

RDFS and an OWL-RL subset ship as rule sets compiled with Ascent (Sahebolamri, Gilray, Micinski, CC 2022), so they run without parsing or planning. They follow the same mode and budget rules as user rules.

## Failure behaviour

What happens when evaluation or maintenance fails. Commits never leave a materialised predicate silently wrong: it is either current or marked stale.

<rule id="dl-mat-crash" strength="must">

Derived-table updates and the `stale` flag commit in the same transaction as the base writes. After a crash, a rule's derived table and flag are consistent with the committed base data; a rebuild that was running restarts from scratch.

</rule>

## Test plan

Tests that check the engine against a reference and that maintenance always agrees with recomputation from scratch.

- Nemo comparison: the Nemo test programs that fit the supported subset give identical results.
- Maintenance property test: random base updates on random stable graphs; after each commit, every non-stale materialised predicate equals a full recomputation.
- Budget test: an update that exceeds the budget marks the rule stale, queries still return correct results, and the rebuild clears the flag.
- Standalone and service equivalence: each query in a corpus returns the same result from `?-` and from `SERVICE <meta:datalog>`.
- Performance: transitive closure and RDFS rule sets against Nemo, [[17-benchmarks#Datalog versus Nemo]] (to be measured).

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in the open-questions file.

- [[open-questions#Hot predicate threshold]]: how overwrite rate is measured and what threshold makes a predicate unstable.
- [[open-questions#Maintenance budget default]]: the default per-transaction budget and whether it is per rule or per transaction.
