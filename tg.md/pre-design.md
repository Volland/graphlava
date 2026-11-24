# Handover: meta-core — mutable single-file metagraph database

> **For Claude Code.** This is a handover from a design session. Your job is to turn it into a full set of **detailed specifications and documentation** under `docs/`. Do **not** start implementation beyond small measurement spikes explicitly listed below. When something is marked **OPEN**, do not decide it silently: write the options and trade-offs into the spec, mark it `OPEN`, and list it in `docs/open-questions.md` for Volodia to decide.

- **Owner:** Volodia (Volodymyr Pavlyshyn)
- **Working name:** `meta-core` (final name **OPEN**)
- **Language:** Rust
- **Sibling project:** tiramemsu (tiramemsu.com) — bitemporal statement graph `(id, s, p, o, tx, op, vts, vte)` on redb/SQLite. meta-core is **standalone** (no shared log format), a leaner, faster, mutable, current-state-only store.

---

## 1. Goals and non-goals

### Goals
1. **Performance first**, optimized for an **agent-memory workload**: many small transactions, frequent **value overwrites**, point lookups, 2–3 hop traversals, star patterns.
2. **Mutable** current state only. **No** tx / op / valid-time columns, no history.
3. **Metagraphs**, all four kinds:
   - statements about statements
   - n-ary hyperedges
   - nested graphs (graphs as nodes, graphs containing graphs)
   - meta-model layer (schema, types, shapes, rules stored as graph)
4. **Query surfaces:** SPARQL 1.2 and a Cypher/GQL subset, both compiled into **one shared IR** (Datalog-based). Datalog also exposed as a third, power-user surface.
5. **Built-ins:** vector index and Datalog reasoning inside the engine.
6. **Platforms:** Linux, macOS, **Windows**, **iOS**, **Android**.
7. **Scale:** 10M – 1B statements per database file.

### Non-goals
- Time travel, audit history, bitemporality (that is tiramemsu's job).
- Distributed / server cluster.
- Full-text search in core (not requested; may be an extension).

---

## 2. Decisions made (treat as requirements)

| # | Topic | Decision |
|---|---|---|
| D1 | Query languages | SPARQL 1.2 + Cypher/GQL subset over **one IR**; IR is extended Datalog; Datalog is also a user surface |
| D2 | Workload | Agent-memory mix; writes are mostly **value overwrites** (set semantics on properties) |
| D3 | Metagraph scope | All four kinds (see Goals) |
| D4 | Statement identity | **Every statement has a u64 ID** |
| D5 | Identity under update | **Same ID, updated in place.** Changing the object keeps the ID; annotations stay attached |
| D6 | Graph column | **No `g` column.** Graphs are nodes; membership is a statement `(stmt_id, :in, graph_id)`; nesting is `(graph, :in, parent_graph)` |
| D7 | Value model | **Hybrid:** RDF terms; literal-valued properties stored **inline** per node; node-valued edges in the orderings |
| D8 | Indexes | **6 orderings** of `(s,p,o,id)` — SPO, SOP, PSO, POS, OSP, OPS — plus `id → (s,p,o)` record |
| D9 | Durability | **Per-transaction choice:** `sync`, `ordered`, `relaxed` |
| D10 | Datalog materialization | **Per rule:** `@materialized` or `@on_demand`; planner picks default from stats when untagged |
| D11 | Built-ins | Vector index + Datalog reasoning in engine |
| D12 | Standalone | No coupling to tiramemsu formats |
| D13 | Platforms | Windows and mobile are first-class |
| D14 | Storage engine | Use a **ready-made KV engine behind a `Storage` trait**; candidates libmdbx / redb / Sanakirja; final pick by bake-off (see §8). Own pager is a later option |
| D15 | Datalog engine | **Build own** Datalog layer compiled to shared IR operators; borrow: Nemo-compatible rule syntax, Datafrog-style kernels, Ascent for built-in compile-time rules |

---

## 3. Data model

### 3.1 Terms and IDs
One 64-bit ID space for everything. High tag bits encode kind:

- IRI
- blank node
- **statement** (statement IDs are ordinary terms → statements about statements are plain triples)
- graph
- hyperedge
- inline literal (small int, bool, date/datetime, short string ≤ 7 bytes, etc. — value lives inside the ID, no dictionary lookup)
- dictionary literal (long strings, typed literals, lang-tagged strings)

Spec must define: exact bit layout, tag table, ordering semantics of inline literals (sort order must match value order where range filters need it), canonicalization of literals, and how IDs are allocated (sequential, per-graph chunks for statements — see §5.2, per-type ranges for hyperedges — see §5.1).

### 3.2 Dictionary
`term → id` and `id → term`. IRI prefix compression (namespace table + local name). Spec: layout, lookup cost, interning during bulk load, garbage collection of unreferenced terms (**OPEN**: eager refcount vs periodic sweep).

### 3.3 Statements
- Key in every ordering: `(a, b, c, id)` where `(a,b,c)` is a permutation of `(s,p,o)`.
- The ID as last key component allows **parallel statements** with the same `(s,p,o)` (Cypher multi-edges) and makes the ID free in every scan.
- `id → (s,p,o)` record for direct ID access.
- Update in place (D5): delete 6 old keys, insert 6 new keys, rewrite the record; ID unchanged.

### 3.4 Inline properties (hybrid model)
- Per node: one packed record of literal-valued properties `node_id → [(pred, literal_id | inline value)…]`.
- Overwriting a value updates **1 tree** instead of 7.
- Inline properties are **virtual triples** to the query layer.
- **Promotion:** when a property occurrence is annotated (statement about it), it is promoted to a full statement with an ID in the orderings. Promoted properties keep their ID on later value updates (D5).
- **Opt-in value indexes** per property key `(pred, value, node)` for filters and lookups by value.
- **OPEN:** demotion when annotations are removed (probably never).

### 3.5 Metagraph mapping
- **Statements about statements:** `(stmt#42, :confidence, 0.9)` — subject is a statement ID.
- **Hyperedges:** dedicated physical structure (§5.1); exposed as role triples `(e, :role, node)`.
- **Nested graphs:** graph IDs are terms; membership via `:in` (§5.2).
- **Meta-model:** reserved system graph holding schema, shapes, hyperedge type definitions (arity, roles), rule definitions, index declarations. The planner reads it.

---

## 4. Physical layout on a KV engine

All structures are B+tree tables inside **one database file** (plus engine lock file if libmdbx).

| Table | Key | Value | Notes |
|---|---|---|---|
| `spo`, `sop`, `pso`, `pos`, `osp`, `ops` | `(a,b,c,id)` | — | 6 orderings |
| `stmt` | `id` | `(s,p,o)` + flags (promoted, annotated) | |
| `props` | `node_id` | packed literal record | hybrid inline properties |
| `propidx_<pred>` | `(value, node_id)` | — | opt-in |
| `hedge` | `e` | `[type, (role,node)…]` | §5.1 |
| `incidence` | `(node, role, type, e)` | — | §5.1 |
| `gchunk` | `chunk_no` | `graph_id` | §5.2 |
| `gmem` / `gmem_rev` | `(g, id)` / `(id, g)` | — | extra memberships |
| `dict_fwd` / `dict_rev` | term / id | id / term | |
| `vec_*` | `(centroid, id)` | PQ code | §7 |
| `meta` | various | various | stats, schema cache, rule state |

### 4.1 Encoding versions
- **v1:** use engine-native duplicate support (libmdbx `DUPFIXED`, redb multimap tables) e.g. key `s|p`, fixed 16-byte values `o|id`. No custom compression.
- **v2:** **block values** — key = first tuple of a block, value = compressed block of ~512 sorted tuples (delta + bitpacking, SIMD decode). Updates rewrite one block.

Spec must define both, with byte layouts, block split/merge rules, and how iterators seek inside blocks.

### 4.2 Size estimate (to validate by benchmark)
Raw ≈ 32 B × 6 orderings + ~32 B record ≈ **224 B/statement**; with 3–5× compression ≈ **45–75 GB per 1B statements**. Spec should refine this per encoding version.

### 4.3 Page size
Target **16 KB** pages where the engine allows (matches Apple Silicon/iOS and 16 KB Android devices; fine on 4 KB hosts). Note Sanakirja uses fixed 4 KB.

---

## 5. Metagraph optimizations

### 5.1 Hyperedges
1. Packed record per hyperedge `e → [type, (role,node)…]`; fixed-width when arity is fixed by the meta-model.
2. **Incidence index** `(node, role, type, e)`.
3. Role triples are **virtual** (served from record + incidence), not stored in the orderings. Write cost of a k-role hyperedge: **1 record + k incidence entries** instead of k × 7.
4. Promotion of a role edge to a full statement only when annotated.
5. **Star-pattern collapse:** planner rewrites `?e :agent ?a ; :target ?t ; :when ?w` into a single scan of relation `hedge_T(e, agent, target, when)`.
6. **Per-type ID ranges** for hyperedges → type scan is a range.
7. Stats: characteristic sets per hyperedge type for cardinality estimation.

### 5.2 Named / nested graphs
1. **Graph-chunked statement IDs:** statement IDs allocated in per-graph chunks (e.g. 2²⁰ IDs per lease). `gchunk` maps chunk → home graph → **zero bytes per statement** for primary membership; graph scan = sequential ID ranges.
2. Extra memberships in `gmem` / `gmem_rev`; exposed as virtual `:in` triples.
3. **Membership statements get IDs lazily** (only when annotated) — **proposed, OPEN** (not yet confirmed).
4. `GRAPH :g { P }` → roaring bitmap of g's IDs (mostly a few ranges) applied as a filter on ordering scans (ID is the last key component) — semi-join, not a join.
5. Nested graphs: `in_star(child, ancestor)` as a `@materialized` rule; ancestor queries expand to the union of descendant ranges.
6. Graph drop = delete chunk ranges + extra memberships.
7. **Opt-in partitioned graphs:** big isolated graphs (per tenant/agent) get their own set of tables in the same file; fast drop, isolation; slower cross-graph queries.
8. Stats: per-graph cardinalities.

---

## 6. Query layer

### 6.1 Shared IR (extended Datalog)
Relational algebra over u64 ID tuples + fixpoint recursion. Must include:
- conjunctive rules, stratified negation, aggregation
- **left join** (SPARQL OPTIONAL, Cypher OPTIONAL MATCH)
- **bag vs set mode** per query/operator
- **edge-isomorphism mode** (Cypher: no repeated relationship within one match)
- different null/type semantics per front end as flags
- virtual relations: inline props, `rdf:reifies`, hyperedge roles, `:in` membership
- vector search operator, callable Datalog predicates

### 6.2 SPARQL 1.2 mapping
- **Reifier = statement ID.** `<< ?s ?p ?o ~ ?r >>` binds `?r` directly from the ordering key — **zero extra joins**. `rdf:reifies` is **virtual**, never stored; triple-term values are built only when projected.
- Annotation syntax `{| … |}` maps to statements about the statement ID.
- Documented **deviations / extensions:**
  1. In-place update: `DELETE` + `INSERT` in one request using the **same reifier** = update in place keeping the ID.
  2. `GRAPH ?g { P }` rewritten to membership filters (direct vs transitive `:in*` semantics — **OPEN**, define both).
  3. Path step that moves from a triple to its reifier and back (paths across meta levels).
  4. Datalog predicates callable from SPARQL.
- Triple terms only in object position (RDF 1.2) — harmless since annotations attach to reifiers.

### 6.3 Cypher/GQL subset mapping
- Node = term; relationship = statement with ID (`-[r]->` binds the ID); properties = inline record; labels = `rdf:type`-like predicate (**spec it**).
- Define the supported subset (MATCH, OPTIONAL MATCH, WHERE, RETURN, WITH, CREATE, MERGE, SET, DELETE, variable-length paths) and what is out of scope.

### 6.4 Datalog surface
- Nemo-compatible syntax where possible (RDF-compatible data model) — do not invent a new dialect.
- Rule annotations `@materialized` / `@on_demand`.
- Reuse oxi-lite's existing parser/stratification work where applicable (repo: https://github.com/Volland/oxilite).

### 6.5 Execution
- **Worst-case optimal joins** (Leapfrog Triejoin) for cyclic patterns — this is what the 6 orderings pay for.
- Merge/hash joins for acyclic patterns.
- Vectorized batches of u64 (e.g. 1024).
- **Late materialization:** decode IDs to strings only at final projection.
- Plan cache + prepared queries (critical for agent-memory latency).
- SIMD with runtime dispatch (AVX2 / NEON) for block decode and set intersection.

### 6.6 Datalog evaluation
- `@on_demand`: magic-sets rewrite + semi-naive evaluation per query.
- `@materialized`: incremental maintenance inside the writer transaction at commit — **counting** for non-recursive rules, **DRed** for recursive ones. Evaluate Differential Dataflow later if overwrite rate makes DRed too slow.
- Untagged rules: planner chooses from stats (fan-out, update rate of body predicates); rules over hot (frequently overwritten) predicates default to on-demand.
- Derived facts stored in the orderings with a derived tag (spec: tagging scheme; whether derived facts get statement IDs — **OPEN**, proposal: lazily).
- Built-in rule sets (e.g. RDFS / OWL-RL subset) via **Ascent** (compile-time).

---

## 7. Vector index
- **Not HNSW** (neighbour-list rewrites amplify copy-on-write writes).
- **IVF-PQ** stored as B+tree entries keyed `(centroid, id)` → page-friendly, MVCC-friendly, memory-light (good for mobile).
- Spec: what gets embedded (nodes, statements, hyperedges), centroid training/retraining, recall/latency targets, integration as an IR operator usable inside recursive Datalog.

---

## 8. Storage engine choice (bake-off)

`Storage` trait abstracts the engine. Candidates:

| Engine | Multi-process | Single file | Notes |
|---|---|---|---|
| **libmdbx** | yes | data file + `-lck` lock file | LMDB-derived; auto grow/shrink (fixes Windows map growth); DUPFIXED; C + bindgen (needs libclang); repo moved off GitHub — vendor source |
| **redb** | **no** (owner process + IPC if needed) | yes | pure Rust, easy mobile builds, savepoints, multimap tables, actively maintained; already used by tiramemsu |
| **Sanakirja** | yes (cross-process locks) | check | Pijul's engine; **O(1) database clone** (reference-counted pages) — useful for agent "what-if" branches / frozen snapshots; 4 KB pages; fixed number of version slots; small community |

Reference numbers from redb's current README (Ryzen 9950X3D, NVMe), key/s or scans/s:

| | redb | lmdb | sqlite |
|---|---|---|---|
| random reads | 1.18M | 1.59M | 370K |
| random range reads | 522K | 933K | 85.5K |
| small batch writes | 79.2K | 70.7K | 37.6K |
| removals | 434K | 361K | 282K |
| compacted size | 650.89 MiB | 741.61 MiB | 562.30 MiB |

Ruled out: RocksDB / fjall / other LSMs (multi-file, no multi-process writers; overwrites need read-before-write so LSM blind-write advantage disappears). SQLite / Turso: viable but 3–6× slower for KV-style access; Turso is beta and is a **competitor baseline**, not a storage layer.

**OPEN (key decision):** is direct multi-process access a hard requirement (→ libmdbx), or is one owner process + IPC acceptable (→ redb)? Concrete case to check: iOS app + extensions writing concurrently.

---

## 9. Transactions, durability, concurrency
- One writer, many snapshot readers (MVCC copy-on-write).
- Durability levels per transaction, mapped per OS:
  - `sync`: Apple `F_FULLFSYNC`; Linux `fsync`/`fdatasync`; Windows `FlushViewOfFile` + `FlushFileBuffers`
  - `ordered`: Apple `F_BARRIERFSYNC`; crash-consistent, cheaper
  - `relaxed`: no flush at commit; made durable by next sync commit
- Spec must define how each level maps onto the chosen engine's sync modes.

---

## 10. Platform requirements
- **Windows:** mmap file growth while mapped (engine-dependent; libmdbx handles it); byte-range locks are **mandatory** → lock bytes at a high offset beyond data (SQLite trick) if implementing own locks.
- **iOS:** virtual address space limits → large files may not map whole (pread/buffer-pool fallback if own pager); **release locks on backgrounding** (apps holding locks on app-group files are killed when suspended, `0xdead10cc`); re-pin readers on resume.
- **Android:** 16 KB page devices; NDK build of engine.
- Little-endian on-disk format everywhere.
- **OPEN:** should one file be portable between desktop and phone? Expected size on phone?

---

## 11. Open questions (must go into `docs/open-questions.md`)
1. Final project name.
2. Multi-process hard requirement vs owner process + IPC (decides libmdbx vs redb).
3. Deleting an annotated statement: cascade delete annotations / block delete / allow dangling.
4. Duplicates: allow parallel statements with same `(s,p,o)` everywhere, or one statement per `(s,p,o)` with optional "multi" mode for Cypher-style graphs.
5. Membership statements: lazy IDs (proposed) vs always IDs.
6. `GRAPH` semantics: direct vs transitive membership.
7. Desktop ↔ mobile file portability and expected mobile sizes.
8. Derived (materialized) facts: get statement IDs lazily or never.
9. Dictionary GC strategy.
10. Property demotion after annotations removed.

---

## 12. Deliverables for Claude Code

Create a `docs/` tree. Each spec must contain: purpose, byte/bit layouts where relevant, invariants, algorithms with complexity, failure/crash behaviour, platform notes, test plan, and a list of OPEN items.

```
docs/
  00-overview.md                 # vision, goals, non-goals, glossary, architecture diagram (mermaid)
  01-data-model.md               # terms, statements, hybrid props, promotion, metagraph semantics
  02-id-encoding.md              # 64-bit layout, tags, inline literals, allocation (chunks, ranges)
  03-dictionary.md
  04-physical-layout.md          # all tables, v1 + v2 encodings, block format, size estimates
  05-storage-trait.md            # Storage trait API, engine adapters (libmdbx, redb, sanakirja)
  06-transactions-durability.md  # MVCC, durability levels per OS/engine, crash recovery
  07-platforms.md                # Windows, macOS, Linux, iOS, Android specifics
  08-hyperedges.md
  09-graphs.md                   # named + nested graphs, chunks, bitmaps, partitioned graphs
  10-ir.md                       # extended Datalog IR: operators, modes, virtual relations
  11-sparql.md                   # SPARQL 1.2 mapping + deviations/extensions
  12-cypher.md                   # supported Cypher/GQL subset + mapping
  13-datalog.md                  # surface syntax, stratification, magic sets, incremental maintenance
  14-planner-execution.md        # cost model, stats, WCOJ, vectorized execution, plan cache
  15-vector-index.md             # IVF-PQ design
  16-api.md                      # Rust embedding API, C FFI, mobile bindings (Swift/Kotlin) outline
  17-benchmarks.md               # bake-off + comparison plan (below)
  18-roadmap.md                  # milestones v0 → v1 → v2
  open-questions.md
  adr/                           # one ADR per decision D1–D15, format: context / decision / consequences
```

Also produce `README.md` (short project intro linking to docs).

### Allowed spikes (measurement only, keep in `spikes/`)
- Engine bake-off harness behind a minimal `Storage` trait: libmdbx vs redb vs Sanakirja.
- Nothing else until specs are reviewed.

---

## 13. Benchmark plan (`docs/17-benchmarks.md`)
1. **Engine bake-off:** real key layout (6 orderings + record, v1 DUPFIXED/multimap style); workload = agent-memory mix: small overwrite transactions, point lookups, 2–3 hop traversals, star patterns; measure p50/p99 latency, throughput, file size, write amplification, at 10M and 100M statements.
2. **vs tiramemsu:** same dataset and workload at **1, 10, 100 versions per fact**. Expectation: close at 1, meta-core pulling ahead as history grows; if not, bottleneck is the query layer.
3. **vs Turso / SQLite:** baseline to beat for agent memory.
4. **Datalog vs Nemo:** materialization speed on standard rule sets (e.g. transitive closure, RDFS).
5. Mobile: run subset on an iOS and an Android device.

---

## 14. References
- redb: https://github.com/cberner/redb
- libmdbx: https://libmdbx.dqdkfa.ru
- Sanakirja: https://docs.rs/sanakirja
- canopydb (single-process alternative, read-modify-write optimized): https://docs.rs/canopydb
- Bf-Tree (design reference): https://docs.rs/bf-tree
- Turso: https://github.com/tursodatabase/turso
- Nemo: https://github.com/knowsys/nemo
- Ascent: https://github.com/s-arash/ascent
- Datafrog: https://github.com/rust-lang/datafrog
- CozoDB (prior art: Datalog over KV + vectors): https://github.com/cozodb/cozo
- RDF 1.2 Concepts: https://www.w3.org/TR/rdf12-concepts/
- oxi-lite (Volodia's Datalog layer): https://github.com/Volland/oxilite
- Papers to cite in specs: Leapfrog Triejoin (Veldhuizen); characteristic sets (Neumann & Moerkotte); DRed / counting for incremental view maintenance; magic sets; IVF-PQ (Jégou et al.).

---

## 15. Working style
- Write specs in precise, implementable terms; prefer tables and byte diagrams over prose.
- Every performance claim in the docs must be either referenced or marked "to be measured".
- Do not resolve OPEN items yourself; present options + recommendation.
- Commit docs in small logical commits.