---
type: Requirement
status: draft
priority: must
specs: ["adr:0003", "adr:0005", "adr:0009", "adr:0010", "adr:0013", "adr:0015", "adr:0007"]
---
# Planner and execution

How a query in the shared IR becomes a physical plan and runs: statistics, cost model, join selection, Leapfrog Triejoin, vectorized batches, late materialization, plan cache and on-demand orderings.

- follows_from:: [[adr:0015]]
- follows_from:: [[adr:0003]]
- depends_on:: [[10-ir#Operators]] {strength: hard}
- depends_on:: [[04-physical-layout#Tables]] {strength: hard}
- introduced_by:: [[18-roadmap#v0]]

## Purpose

The planner turns one IR program into a physical plan over the orderings, `props`, hyperedge records and graph ranges. The goal is low latency for agent-memory queries: point lookups, 2–3 hop traversals and star patterns.

Scope in v0: statistics, cost-based join ordering, Leapfrog Triejoin, merge and hash joins, vectorized operators, late materialization, plan cache. SIMD block decode and hyperedge star-pattern collapse arrive in v2 ([[18-roadmap#v2]]).

<non-goal>

Adaptive re-optimisation during execution, parallel intra-query execution and a cost model for remote `SERVICE` calls are not in v0.

</non-goal>

## Statistics

Statistics the planner reads to estimate cardinalities and choose plans. All are kept in the `meta` table, updated incrementally at commit, and rebuilt by a background pass when drift exceeds a threshold.

- follows_from:: [[adr:0009]]
- depends_on:: [[09-graphs#Chunk membership]] {strength: soft}
- depends_on:: [[08-hyperedges#Record format]] {strength: soft}

| Statistic | Key | Content | Updated |
| --- | --- | --- | --- |
| Predicate counts | `p` | statements, distinct `s`, distinct `o` | at commit (exact counts, approximate distincts via HyperLogLog) |
| Characteristic sets | set of predicates of a subject | number of subjects with exactly that set, per-predicate multiplicity | background pass |
| Hyperedge type stats | hyperedge type `T` | count, per-role distinct nodes | at commit |
| Graph stats | graph `g` | statement count, chunk count, descendant count | at commit |
| Overwrite rate | predicate `p` | exponentially decayed overwrites per second | at commit |
| Value index usage | `(p, lookup shape)` | decayed count of value-driven lookups | at plan time |
| Ordering usage | variable order | decayed count of plans that wanted a missing ordering | at plan time |

<rule id="plan-charsets" strength="must">

Star-pattern cardinality is estimated with characteristic sets (Neumann and Moerkotte, ICDE 2011): a star `?s p1 ?o1 ; p2 ?o2 ...` is estimated from the subjects whose characteristic set contains all its predicates, not from the product of per-predicate selectivities.

</rule>

<rule id="plan-hot-predicate" strength="must">

A predicate is hot when its decayed overwrite rate exceeds the configured threshold. Hot predicates are reported to the Datalog layer, which rejects `@materialized` rules over them unless forced ([[13-datalog#Materialized maintenance]]).

</rule>

<rule id="plan-stats-no-block" strength="must">

Statistics updates at commit are O(number of changed statements) and never scan. Characteristic sets are recomputed only by the background pass.

</rule>

The background pass costs one scan of SPO, O(N) in statements; its runtime at 100M statements is to be measured.

## Join selection

How the planner picks a join algorithm and a scope plan for each IR conjunction. Cyclic patterns go to Leapfrog Triejoin; acyclic ones to merge or hash joins; graph scopes pick one of three plans.

- follows_from:: [[adr:0003]]
- follows_from:: [[adr:0007]]
- depends_on:: [[09-graphs#Scope expansion]] {strength: hard}
- measured_by:: [[17-benchmarks#Engine bake-off]]

<rule id="plan-join-choice" strength="must">

A conjunction whose join hypergraph is cyclic (for example a triangle) is planned with Leapfrog Triejoin when a fitting variable order exists. An acyclic conjunction is planned as a tree of merge joins where both inputs arrive sorted on the join variable, and hash joins otherwise.

</rule>

<rule id="plan-scope-choice" strength="must">

For `GRAPH :g { P }` the planner chooses one of three plans by estimated cost: range-driven (read the scope's statement-ID ranges from `stmt`, then probe orderings), filter-driven (scan an ordering and filter `id` against the scope's roaring bitmap) or partitioned (run `P` against the graph's own tables). The scope is the transitive closure by default ([[11-sparql#Graph semantics]]).

</rule>

| Plan | Cost | Chosen when |
| --- | --- | --- |
| Range-driven | O(scope size) sequential `stmt` reads plus probes | scope is small relative to the pattern's ordering range |
| Filter-driven | O(ordering range) with an O(1) bitmap test per row | the pattern is selective and the scope is large |
| Partitioned | same as an unscoped query on the partition | the graph is a partitioned graph |

<rule id="plan-union-sources" strength="must">

A pattern with an unbound predicate reads both the orderings and the inline property records, and the union is deduplicated by statement identity. A pattern with a bound predicate reads only the source its object kind routes to ([[01-data-model#Inline properties]]).

</rule>

## Leapfrog Triejoin

Worst-case optimal join for cyclic patterns (Veldhuizen, ICDT 2014), run over trie iterators built on the orderings. Each join variable is intersected across all relations that mention it.

- follows_from:: [[adr:0015]]
- depends_on:: [[04-physical-layout#Key byte order]] {strength: hard}
- measured_by:: [[17-benchmarks#Ordering count comparison]]

<rule id="plan-ltj-iterator" strength="must">

Every ordering exposes a trie iterator with `open`, `up`, `next`, `seek(key)` and `at_end`, over keys `(a, b, c)` with the statement ID as the value. `seek` costs O(log n) in the B+tree and amortised O(1) when it lands on the same leaf.

</rule>

<rule id="plan-ltj-order" strength="must">

The planner chooses a global variable order for which every atom has an ordering whose key levels follow that order; a constant at a key level is handled by a `seek` at that level for each prefix. With SPO, POS and OSP always present, every atom with one or two bound positions has a fitting ordering. When no global order fits, the planner falls back to a merge or hash join plan and records the missing ordering in the usage statistics.

</rule>

Running time is O(N^(ρ*)) for the fractional edge cover number ρ* of the query (Veldhuizen 2014); for a triangle over N edges that is O(N^1.5). Absolute numbers are to be measured.

<example>

<input>

Triangle `?a :knows ?b . ?b :knows ?c . ?c :knows ?a` with orderings SPO, POS, OSP only.

</input>

<output>

Variable order `a, b, c`. Atom `?a :knows ?b` uses SPO: open level `s` for `a`, seek the constant `:knows` at level `p`, then level `o` for `b`. Atom `?b :knows ?c` uses SPO the same way. Atom `?c :knows ?a` needs `a` before `c`, so it uses POS `(:knows, a, c)`. All three atoms fit. With PSO present, the first two atoms avoid one seek per `a` and `b` value; the planner counts that as a wanted ordering ([[14-planner-execution#On-demand orderings]]).

</output>

</example>

## Vectorized execution

Operators exchange batches of up to 1024 `u64` IDs per column, not single tuples. Strings are decoded only at final projection (late materialization).

- depends_on:: [[02-id-encoding#Inline literals]] {strength: hard}
- measured_by:: [[17-benchmarks#Engine bake-off]]

<rule id="plan-batch" strength="must">

Every physical operator consumes and produces columnar batches of at most 1024 rows of `u64`. Filters produce a selection vector instead of copying.

</rule>

<rule id="plan-late-materialize" strength="must">

Dictionary lookups (`id -> term`) happen only in the final projection, in `ORDER BY` on dictionary literals, and in filters that need a lexical value. Inline literals are compared and ordered without decoding.

</rule>

<rule id="plan-simd-dispatch" strength="should">

Set intersection and v2 block decode use SIMD with runtime dispatch: AVX2 on x86-64, NEON on aarch64, and a scalar fallback. The choice is made once per process. SIMD block decode is v2 ([[04-physical-layout#Encoding v2]]).

</rule>

## Plan cache

Prepared queries and a plan cache in the owner process, so repeated agent-memory queries skip parsing and planning. Plans are keyed by normalised IR and invalidated by schema or statistics changes.

- depends_on:: [[16-api#Rust API]] {strength: soft}
- depends_on:: [[13-datalog#Surface syntax]] {strength: hard}

<rule id="plan-one-pipeline" strength="must">

SPARQL queries and standalone Datalog programs ([[13-datalog#Surface syntax]]) are compiled into the same IR and enter the same planner and plan cache. A prepared Datalog program is cached and re-bound exactly like a prepared SPARQL query.

</rule>
- measured_by:: [[17-benchmarks#Engine bake-off]]

<rule id="plan-cache-key" strength="must">

The cache key is the normalised IR (constants replaced by parameters), the scope mode and the set of existing orderings and value indexes. A prepared query binds parameters without re-planning.

</rule>

<rule id="plan-cache-invalidate" strength="must">

A cached plan is invalidated when an ordering or value index is created or dropped, when a rule is added or marked stale, or when the statistics of a relation it uses drift by more than a configured factor (default 4x).

</rule>

## On-demand orderings

PSO, SOP and OPS exist only when the planner or the meta-model asks for them. They are built online and can be dropped.

- follows_from:: [[adr:0015]]
- follows_from:: [[adr:0005]]
- depends_on:: [[05-storage-trait#Trait API]] {strength: hard}
- measured_by:: [[17-benchmarks#Ordering count comparison]]

<rule id="plan-ordering-build" strength="must">

When the decayed count of plans that wanted a missing ordering exceeds a threshold, or the meta-model declares it, the engine builds it in the background in bounded batches between write transactions, then marks it ready in `meta` in a single commit. Writes made during the build are applied to the new ordering by the writer once the build has started.

</rule>

<rule id="plan-ordering-drop" strength="should">

An on-demand ordering unused for a configured period (default 7 days of owner-process uptime) is dropped, unless declared in the meta-model.

</rule>

The same mechanism builds value indexes ([[01-data-model#Inline properties]]). Build cost is one scan of SPO plus sorted inserts, O(N log N); time and file growth at 100M statements are to be measured.

## Failure behaviour

The planner and executor hold no durable state of their own except statistics and ordering build progress in `meta`.

<rule id="plan-build-crash" strength="must">

A crash during an ordering build leaves it marked `building`; on open, the engine drops the partial ordering and restarts the build if it is still wanted. A query never reads an ordering that is not marked ready.

</rule>

## Platform notes

On mobile the background statistics pass and ordering builds run only in the foreground or on charge, the same policy as the dictionary sweep ([[03-dictionary#Sweep]]). SIMD dispatch covers NEON on iOS and Android arm64 and AVX2 on desktop x86-64.

## Test plan

Tests for plans and execution, run against a reference evaluator that uses nested loops.

- Differential test: random BGPs over a random graph, each planned plan versus a naive evaluator, for all scope modes.
- Leapfrog property test: intersection results equal set intersection for random sorted inputs.
- Plan cache: invalidation after index creation, rule staleness and statistics drift.
- Crash test: kill during an ordering build, reopen, check the ordering is absent or rebuilt.
- Benchmarks: [[17-benchmarks#Engine bake-off]], [[17-benchmarks#Ordering count comparison]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Missing ordering for cyclic patterns]]: whether a cyclic query that needs a missing ordering waits for, triggers, or never triggers a synchronous build.
- [[open-questions#Statistics drift threshold]]: the drift factor that invalidates cached plans and triggers a statistics rebuild.
