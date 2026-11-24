---
type: Requirement
openspec: [transactions]
status: draft
priority: must
specs: ["adr:0002", "adr:0006", "adr:0012", "adr:0018", "adr:0025", "adr:0026"]
---
# Transactions and durability

How meta-core runs transactions on redb: one writer and many snapshot readers in the owner process, three durability levels per transaction, group commit, crash recovery, and cascade delete.

- follows_from:: [[adr:0002]]
- follows_from:: [[adr:0012]]
- follows_from:: [[adr:0025]]
- follows_from:: [[adr:0006]]
- depends_on:: [[05-storage-trait#Trait API]] {strength: hard}
- depends_on:: [[02-id-encoding#Allocation]] {strength: hard}

## Purpose

Agent-memory workloads issue many small write transactions. This spec fixes what a commit guarantees, how cheap durable commits are made through batching, and what a crash may lose.

## Transaction model

One owner process holds the file ([[adr:0002]]). Inside it, one write transaction runs at a time and any number of read transactions read consistent snapshots.

<rule id="tx-single-writer" strength="must">

At most one write transaction is open per database file. Write requests from all clients queue in the owner process and run in arrival order. A write transaction sees its own uncommitted writes. Only code running in the owner process holds a write transaction; IPC and server clients submit one-shot write requests ([[16-api#Write requests]], [[adr:0026]]).

</rule>

<rule id="tx-snapshot-read" strength="must">

A read transaction sees exactly the state of the last commit that finished before it started, for its whole life, regardless of later commits. Reads never block the writer and the writer never blocks reads.

</rule>

<rule id="tx-reader-age" strength="should">

A read transaction older than a configurable limit (default 60 s, to be tuned by measurement) is reported to the caller and may be cancelled, because a pinned snapshot stops redb from reusing freed pages and the file grows.

</rule>

<rule id="tx-atomic" strength="must">

A write transaction is atomic: every table it touches (orderings, `stmt`, `props`, `hedge`, `incidence`, `gchunk`, `gmem`, dictionary, `meta` counters, derived tables) changes together or not at all.

</rule>

### Write transaction cost

The leading paragraph lists what one base statement write touches; numbers are to be measured in [[17-benchmarks#Engine bake-off]].

| Operation | Trees touched |
| --- | --- |
| insert node-valued statement | 3 orderings + `stmt` (+ dictionary for new terms) |
| overwrite inline property | `props` (+ value index if present) |
| overwrite object of a statement keeping its ID | 3 old keys deleted, 3 new keys inserted, `stmt` rewritten |
| insert k-role hyperedge | `hedge` + k `incidence` entries ([[08-hyperedges#Record format]]) |

## Durability levels

Each write transaction names one of three durability levels. The levels are defined by what they guarantee at acknowledgement, not by the OS call used ([[adr:0012]]).

<rule id="tx-durability-default" strength="must">

The default durability level is `ordered`. A caller may choose `sync` or `relaxed` per transaction.

</rule>

| Level | Guarantee when the commit call returns | redb mapping | Typical use |
| --- | --- | --- | --- |
| `sync` | the transaction survives any crash or power loss | runs in the open batch and closes it at once with `Durability::Immediate` | user-visible saves, checkpoints |
| `ordered` | the transaction survives any crash; the call returns only after its batch is flushed | runs in the open batch; the batch commits once with `Durability::Immediate` | default for agent memory writes |
| `relaxed` | the transaction is atomic and crash-consistent but may be lost; a later `sync` or `ordered` batch makes it durable | runs in the open batch; a batch of only `relaxed` requests commits with `Durability::None` | scratch state, caches, bulk load |

<rule id="tx-durability-prefix" strength="must">

Durability is prefix-ordered: if a crash keeps transaction T, it keeps every transaction committed before T. A crash can only lose a suffix of `relaxed` or not-yet-acknowledged `ordered` transactions.

</rule>

<rule id="tx-durability-verify" strength="must">

The mapping above relies on redb guaranteeing that a `Durability::Immediate` commit is durable when it returns and makes every earlier `Durability::None` commit durable, and that a crash after a `None` commit recovers to a consistent earlier state. `None` is used only for batches of `relaxed` requests. These properties are verified by the engine spike before v0 ([[17-benchmarks#Engine bake-off]]).

</rule>

<non-goal>

Mapping levels to specific OS primitives (`F_FULLFSYNC`, `F_BARRIERFSYNC`, `FlushFileBuffers`). redb chooses its own sync call per platform; [[07-platforms#macOS]] records what it must use.

</non-goal>

## Group commit

Write requests are batched into one engine write transaction, which commits once, so pages shared by the batch are copied once and a batch becomes visible exactly when it is durable ([[adr:0025]]).

<rule id="tx-group-one-txn" strength="must">

A batch is one engine write transaction. The writer opens it for the first request it dequeues and runs every following request of the batch inside it, in arrival order. The engine commits once per batch.

</rule>

<rule id="tx-group-window" strength="must">

A batch that holds an `ordered` request closes when it is N ms old or holds M bytes of changes, whichever comes first. A batch of only `relaxed` requests closes as soon as the request queue is empty. N and M are configurable; defaults are to be measured (starting points 2 ms and 4 MiB).

</rule>

<rule id="tx-group-close" strength="must">

A batch closes early when a `sync` request has run in it, when the owner process is asked to go to the background ([[07-platforms#iOS]]), or when the database is closed.

</rule>

<rule id="tx-group-request-isolation" strength="must">

A request in a batch first checks everything that can fail by reading (update conflicts, compare-and-set mismatches, hyperedge shapes, the cascade limit, meta-model validation) and then applies its writes through a per-request overlay that is merged into the batch transaction only when the request succeeds. A request that fails is reported to its caller and leaves the batch unchanged. Later requests of the batch see the writes of earlier successful ones.

</rule>

<rule id="tx-group-ack" strength="must">

Every caller in a batch is acknowledged after the batch commit returns. If the commit fails, every request in the batch is reported as failed with an I/O error, the database is marked read-only, and the owner process must reopen it.

</rule>

### Algorithm

The writer loop runs requests in order inside the open batch and commits once per batch. Cost: one engine commit and one flush per batch instead of one commit per request.

1. Take the next request from the queue. If no batch is open, begin a redb write transaction for a new batch.
2. Run the request under `tx-group-request-isolation`; on failure, report it and continue with the next request.
3. Append the caller to the batch with its durability level.
4. If the request was `sync`, or a window or close rule fires, close the batch: commit with `Immediate` if the batch holds any `sync` or `ordered` request, else with `None`; then acknowledge every caller of the batch in arrival order.
5. A timer closes an open batch when its window expires with no further requests.

<example>

<input>

Three agents each write one `ordered` statement 0.3 ms apart; N = 2 ms.

</input>

<output>

One engine transaction holds all three requests and commits once with `Immediate` about 2 ms after the first. All three callers return after that flush. Latency per caller is up to N plus one flush; commits and flushes per second drop by the batch size, and B-tree pages touched by more than one request are copied once.

</output>

</example>

### Visibility before durability

A batch becomes visible to new readers when its engine commit returns, which for a batch with any `sync` or `ordered` request is also when it is durable ([[adr:0025]]).

Readers therefore never observe `sync` or `ordered` data that a crash can lose. Only a batch of `relaxed` requests is visible before it is durable.

<rule id="tx-visible-durable" strength="must">

A read transaction never sees a `sync` or `ordered` request before that request is durable.

</rule>

## Crash recovery

After a crash, opening the file returns it to the last durable commit. No meta-core-level log exists; redb's copy-on-write commit protocol is the only recovery mechanism.

<rule id="tx-recovery-state" strength="must">

After recovery the database equals the state after some durable commit C, and every acknowledged `sync` or `ordered` transaction is at or before C. Allocator counters, `gchunk`, derived tables and rule staleness flags are part of that state, so they are consistent with each other ([[02-id-encoding#Allocation]]).

</rule>

<rule id="tx-recovery-volatile" strength="must">

State held only in the owner process (open batch, plan cache, statistics deltas not yet written, sweep cursor) is rebuilt or discarded on open. Nothing durable depends on it.

</rule>

<rule id="tx-recovery-format" strength="must">

On open, the format version and feature flags in `meta` are checked before any other read; an unknown flag refuses the open ([[adr:0018]]).

</rule>

redb may repair its allocator state after an unclean shutdown, and the time this takes on a large file is to be measured; see [[open-questions#Crash repair time on large files]].

## Cascade delete

Deleting a statement deletes, in the same transaction, every statement that annotates it, transitively; IDs are never reused ([[adr:0006]]).

- depends_on:: [[01-data-model#Annotations]] {strength: hard}
- depends_on:: [[08-hyperedges#Incidence index]] {strength: hard}

<rule id="tx-cascade-annotations" strength="must">

Deleting item X (a statement, a promoted property, a promoted membership or a hyperedge) deletes every statement whose subject is X, every inline property stored on X as a node, and every `gmem` entry of X, then repeats for each deleted statement.

</rule>

<rule id="tx-cascade-hyperedge-block" strength="must">

If X fills a role in any hyperedge (an `incidence` entry with node X exists), the delete fails with `ReferencedByHyperedge` and the transaction changes nothing, unless the caller passes `DETACH`. With `DETACH`, each such hyperedge is deleted, and its deletion cascades in turn.

</rule>

<rule id="tx-cascade-keep" strength="must">

With `KEEP ANNOTATIONS`, only X is deleted. Statements whose subject is X remain, are reachable by their own IDs and by scans, and their subject ID resolves to no statement.

</rule>

<rule id="tx-cascade-limit" strength="must">

A cascade that would delete more than a per-transaction limit (default 100,000 items, configurable) fails with `CascadeLimitExceeded` and rolls back. The caller may raise the limit for one transaction.

</rule>

<rule id="tx-cascade-overwrite" strength="must">

Overwriting the object of a statement keeps its ID and deletes nothing else; annotations stay attached.

</rule>

### Algorithm

Breadth-first over subjects, using the SPO ordering prefix and the `props` record of each deleted item. Cost: O(d · log n) for d deleted items, plus one `incidence` prefix probe per item.

1. Push X on a work queue; mark it.
2. Pop Y. Probe `incidence` with prefix `(Y)`; if non-empty and not `DETACH`, abort. With `DETACH`, push each hyperedge found.
3. Scan SPO with prefix `(Y)`; push each statement ID found. Delete the `props` record of Y. Delete `gmem` and `gmem_rev` entries of Y.
4. Delete Y's own keys (3 orderings and `stmt`, or `hedge` and `incidence`); count it against the limit.
5. Repeat until the queue is empty.

### References from the object position

A statement whose object is X (`(:note, :about, stmt#42)`) is not an annotation and is not deleted by rule `tx-cascade-annotations`. After the delete its object is a dangling ID. Whether to cascade, block or keep is [[open-questions#Delete with object-position references]].

## Platform notes

Durability calls differ by OS and are redb's choice; [[07-platforms#iOS]] requires the open batch to be flushed and file locks released before the app is suspended.

## Test plan

Tests are written against the rules above; crash tests run on all five platforms in CI where devices allow.

- Crash injection: kill the owner process at random points of the writer loop; check `tx-recovery-state` and `tx-durability-prefix` over 10^4 runs per level.
- Power-loss simulation on Linux with a fault-injecting block device (dm-flakey or similar) for `sync` and `ordered`.
- Group commit: acknowledged `ordered` transactions are present after a crash that happens right after acknowledgement; a reader started during an open batch does not see its requests (`tx-visible-durable`).
- Request isolation: a batch where every third request fails a compare-and-set or the cascade limit commits exactly the other requests, and later requests see earlier successful ones.
- Write amplification: bytes written per request at batch sizes 1, 8 and 64 ([[17-benchmarks#Durability commit latency]]).
- Cascade: chains of annotations of depth 1..10, hyperedge block and `DETACH`, `KEEP ANNOTATIONS`, limit rollback.
- Reader isolation: long reader sees a stable snapshot while 10^5 commits run.
- Benchmark: commit latency and flushes per second per level in [[17-benchmarks#Engine bake-off]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Crash repair time on large files]]
- [[open-questions#Delete with object-position references]]
