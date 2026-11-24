---
type: Requirement
status: draft
priority: must
specs: ["adr:0002", "adr:0012", "adr:0018"]
---
# Storage trait

The Rust trait that hides the KV engine from the rest of graphlava, its transaction and iterator contract, and the redb adapter that implements it in v0. Bake-off adapters for libmdbx and Sanakirja exist only in spikes.

- follows_from:: [[adr:0002]]
- follows_from:: [[adr:0012]]
- depends_on:: [[04-physical-layout#Key byte order]] {strength: hard}

## Purpose

Every layer above storage talks to this trait, never to redb. That keeps an own pager possible later and lets the bake-off run the same workload on several engines ([[17-benchmarks#Engine bake-off]]).

<non-goal>

Hiding engine performance differences. The trait fixes semantics, not speed; callers may rely on the cost model stated per method.

</non-goal>

## Trait API

The trait models one database file with one writer and many snapshot readers. Tables are opened by a static descriptor; keys and values are byte slices whose widths the descriptor declares.

```rust
pub trait Storage: Send + Sync + 'static {
    type Read<'a>: ReadTxn where Self: 'a;
    type Write<'a>: WriteTxn where Self: 'a;

    fn open(path: &Path, opts: &OpenOptions) -> Result<Self> where Self: Sized;
    fn begin_read(&self) -> Result<Self::Read<'_>>;
    fn begin_write(&self) -> Result<Self::Write<'_>>;     // blocks while another writer is active
    fn compact(&mut self) -> Result<CompactReport>;        // needs exclusive access
    fn file_size(&self) -> Result<u64>;
}

pub struct TableDef {
    pub name: &'static str,          // dynamic names (partitions, drv_*, vec_*) use TableName
    pub key_width: Width,            // Fixed(n) or Var
    pub value_width: Width,
}

pub trait ReadTxn {
    type Table<'t>: ReadTable where Self: 't;
    fn table(&self, def: &TableName) -> Result<Option<Self::Table<'_>>>;
    fn snapshot_id(&self) -> u64;                          // commit counter this view sees
}

pub trait ReadTable {
    type Iter<'i>: Cursor where Self: 'i;
    fn get(&self, key: &[u8]) -> Result<Option<Bytes<'_>>>;
    fn range(&self, lo: Bound<&[u8]>, hi: Bound<&[u8]>) -> Result<Self::Iter<'_>>;
    fn len_estimate(&self) -> Result<u64>;
}

pub trait Cursor {
    fn next(&mut self) -> Option<Result<(Bytes<'_>, Bytes<'_>)>>;
    fn seek(&mut self, key: &[u8]) -> Result<()>;          // first entry >= key, forward only
    fn next_back(&mut self) -> Option<Result<(Bytes<'_>, Bytes<'_>)>>;
}

pub trait WriteTxn: ReadTxn {
    type TableMut<'t>: WriteTable where Self: 't;
    fn table_mut(&mut self, def: &TableName) -> Result<Self::TableMut<'_>>;  // creates if absent
    fn drop_table(&mut self, def: &TableName) -> Result<bool>;
    fn commit(self, durability: EngineDurability) -> Result<CommitInfo>;
    fn abort(self) -> Result<()>;
}

pub trait WriteTable: ReadTable {
    fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<Option<OwnedBytes>>;
    fn remove(&mut self, key: &[u8]) -> Result<Option<OwnedBytes>>;
    fn remove_range(&mut self, lo: Bound<&[u8]>, hi: Bound<&[u8]>) -> Result<u64>;
}

pub enum EngineDurability { Flush, NoFlush }
```

<rule id="st-single-writer" strength="must">

At most one write transaction exists at a time per open database. `begin_write` blocks until the current writer commits or aborts.

</rule>

<rule id="st-snapshot" strength="must">

A read transaction sees exactly the state of one committed write transaction for its whole life, regardless of later commits. Readers never block the writer and the writer never blocks readers.

</rule>

<rule id="st-order" strength="must">

Keys in every table are ordered by unsigned byte-wise comparison. `range` and `Cursor::seek` follow that order ([[04-physical-layout#Key byte order]]).

</rule>

<rule id="st-durability-map" strength="must">

`commit(Flush)` returns only after the commit is durable on the device. `commit(NoFlush)` returns after the commit is atomic and visible to new readers, and becomes durable no later than the next `Flush` commit. The three user-facing **Durability levels** are built on these two in [[06-transactions-durability#Durability levels]].

</rule>

<rule id="st-crash-atomic" strength="must">

After a crash, opening the file yields the state of the last commit that returned from `commit(Flush)`, or a later committed state. No partial transaction is ever visible.

</rule>

<rule id="st-no-engine-types" strength="must">

No engine type appears in the trait's public signatures. Crates above `graphlava-storage` depend only on the trait.

</rule>

### Cost model per method

The planner and executor assume these costs; adapters must document deviations. Absolute numbers are to be measured.

| Method | Expected cost |
| --- | --- |
| `get` | O(log n) page reads |
| `range` + k × `next` | O(log n + k) |
| `Cursor::seek` forward by d entries | O(log d) amortised, or O(log n) worst case |
| `insert`, `remove` | O(log n) page writes, copy-on-write of the path |
| `remove_range` of k entries | O(log n + k) |

### Errors

The trait reports a small closed set of errors so callers can react without knowing the engine.

| Error | Meaning | Caller reaction |
| --- | --- | --- |
| `Io` | OS I/O failure | abort transaction; surface to user |
| `Corrupted` | checksum or structure failure | refuse to open; offer recovery tool |
| `Full` | disk or address space exhausted | abort; on iOS see [[07-platforms#iOS]] |
| `FormatMismatch` | unknown format version or feature flag | refuse to open ([[adr:0018]]) |
| `Locked` | file held by another owner process | refuse to open ([[adr:0002]]) |

## redb adapter

The v0 adapter maps the trait onto redb. It is the only crate that depends on redb.

- follows_from:: [[adr:0002]]

| Trait concept | redb mapping |
| --- | --- |
| `Storage::open` | `redb::Builder` with cache size from `OpenOptions` |
| `ReadTxn` | `ReadTransaction` |
| `WriteTxn` | `WriteTransaction` |
| table with fixed widths | `TableDefinition<&[u8; K], &[u8; V]>` (fixed-size byte arrays) |
| table with variable widths | `TableDefinition<&[u8], &[u8]>` |
| `EngineDurability::Flush` | `Durability::Immediate` |
| `EngineDurability::NoFlush` | `Durability::None` |
| `compact` | `Database::compact` |
| `Locked` | redb's single-open check on the file |

<rule id="st-redb-durability-verify" strength="must">

The mapping of `Flush` and `NoFlush` to redb durability modes, and the OS sync call redb issues for `Immediate` on each platform (for example `F_FULLFSYNC` on Apple), must be verified against the redb version pinned in `Cargo.lock` before v0 ships. Until then the mapping in the table above is provisional.

</rule>

<rule id="st-redb-pinned" strength="must">

The redb version is pinned exactly. Upgrading it is a reviewed change that reruns the golden-file tests of [[04-physical-layout#Test plan]], because a redb file-format change is a graphlava format change ([[adr:0018]]).

</rule>

### Things redb does not provide

Some handover assumptions do not hold on redb. Each is handled above the trait or left open.

| Handover assumption | On redb | Handling |
| --- | --- | --- |
| 16 KB pages (§4.3) | page size not confirmed configurable | measured in the bake-off ([[adr:0035]]) |
| `ordered` durability as a barrier fsync | no barrier mode | group commit, [[06-transactions-durability#Group commit]] |
| multi-process access | single process only | owner process, [[adr:0002]] |
| native value compression | none | v2 blocks in the value layer, [[04-physical-layout#Encoding v2]] |
| O(1) database clone | none | not offered; Sanakirja remains a deferred option in [[adr:0002]] |
| savepoint inside a transaction that has already written | not available, as far as known (verified in the spike) | per-request overlays inside one batch transaction ([[06-transactions-durability#Group commit]], [[adr:0025]]) |
| fast reopen after an unclean shutdown | repair walks the file; time grows with size (to be measured) | [[resolved-questions#Crash repair time on large files]] |

### Spike adapters

libmdbx and Sanakirja adapters exist only under `spikes/` for the bake-off. They implement the same trait and the same conformance tests, and are not shipped.

## Failure behaviour

The adapter adds no state of its own outside redb, so crash behaviour is redb's. Every trait method is cancellation-safe: dropping a write transaction without `commit` aborts it.

## Platform notes

redb is pure Rust and builds for all five targets without a C toolchain. Platform effects of mmap, file growth and file locks are in [[07-platforms]]. On iOS the adapter must close or release the database on backgrounding as [[07-platforms#iOS]] requires.

## Test plan

One conformance suite runs against every adapter, so the trait's rules mean the same thing for each engine.

- Conformance: every rule above has a test, including snapshot isolation under a concurrent writer and range order with random keys.
- Crash: a fault-injecting file wrapper drops writes after each sync point; reopening must satisfy `st-crash-atomic`.
- Durability: power-cut tests on real devices for `Flush` (manual, per platform, before v0 release).
- Benchmark: the bake-off workload of [[17-benchmarks#Engine bake-off]] through the trait.

## Open items

No questions remain open; the ones this spec raised are answered in [[resolved-questions]].

