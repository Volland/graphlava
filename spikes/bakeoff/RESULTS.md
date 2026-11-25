# Storage engine bake-off: results

Spike for tg.md/18-roadmap.md "Spike: engine bake-off", run against tg.md/17-benchmarks.md "Engine bake-off", "Ordering count comparison" and "Harness and method". Everything ran on one Apple M2 Max laptop on 2026-11-24. Raw results are in `results/*.json`, the harness is in `src/`, and `summarize.py` regenerates the appendix tables.

## Answers

| Question | Answer |
| --- | --- |
| Does redb pass the gate? The rule: redb fails only if another engine is more than 2x better on p99 of the mix or on file size | **Pass, with a narrow margin on one axis.** File size: redb 411 B/stmt, libmdbx 417, Sanakirja 725 (logical) or 1,396 (allocated). Nobody beats redb. Sync mix p99: redb 8.0-12.9 ms, libmdbx 7.8 ms in its own `DURABLE` mode (20.8 ms as commit + `env_sync`), Sanakirja 34.9 ms. All are `F_FULLFSYNC`-bound, so this is a tie. Relaxed mix p99: libmdbx 288-292 µs and redb 453 µs (stable rerun), so libmdbx is **1.55x** better, under the 2x line. redb's first 5-run set gave 613 µs (2.1x) because 2 of its runs were disturbed by background I/O; see "Gate numbers" in A. |
| Does ADR 0015 hold? The rule: three orderings write at least 30% faster and are at least 30% smaller, and cyclic queries after on-demand builds are within 1.5x of six | **Size: yes**, 38.8% smaller (411 vs 672 B/stmt) and 41% fewer bytes written per write. **Relaxed write latency: yes**, p50 40% lower and p99 43% lower. **Sync write latency: no**, p50 only 2% and p99 12% lower, because one `F_FULLFSYNC` (about 5 ms) dominates. **Cyclic queries: yes, and without building anything.** The best 3-ordering plans equal the 6-ordering layout (best-plan ratio 3/6 is 0.95-0.98 on all four queries), and the plans that use PSO/SOP/OPS were not faster |
| Crash-repair time at 10M and 100M (redb 2.6.4, warm cache) | **No quick-repair:** any `kill -9` after the database was opened writable forces a full repair on the next open. It takes **3.4 s** (no commit since open) to **15.5 s** at 10M (4.1 GB), and **105-221 s** at 100M (43 GB), with about 1.2 GB RSS during repair. Clean open is 5-7 ms at both sizes. **With `set_quick_repair(true)`** the repaired open is **9-12 ms** at both sizes, but durable commits cost about 2x (2-phase commit plus allocator state). Warm cache for 10M, partly warm for 100M |
| Does redb 2.6.4 allow a savepoint after the write txn has written? | **No.** `ephemeral_savepoint()` and `persistent_savepoint()` both return `Err(SavepointError::InvalidSavepoint)`, Display "Savepoint is invalid or cannot be created.". **Opening a table without writing is already enough to make it fail.** Only a savepoint taken first in the transaction works |
| Does a `Durability::Immediate` commit make earlier `None` commits durable? | **Yes.** It is shown in the source, and a `kill -9` test passed 3 of 3. But a `None` commit is lost on a plain *process* kill if no Immediate follows. `None` commits never free pages, so the file grows without bound unless durable commits are interleaved |
| Sync call redb uses on macOS | `Immediate` calls `File::sync_data()`, which Rust std implements on Apple as `fcntl(F_FULLFSYNC)`. `Eventual` calls `fcntl(F_BARRIERFSYNC)` directly (`file_backend/unix.rs:66-83`) |
| Would redb 4.x change the conclusions? | **It would not change the gate verdict, but it would remove most of redb's weak spots.** This was measured cheaply with the same adapter and 10M data: 5 latency runs, 1 throughput run and the probe. Relaxed mix p99 is **167 µs** (161-172), which beats libmdbx's 288 µs; write p50 is 58 µs, against 175 µs on 2.6.4. File after load is 257 B/stmt allocated (429 B by length), against 411. Bulk load takes 97 s, against 129 s. 100k `None`-only overwrites grew the file by only 0.2 GB, against 5.9 GB. 16 readers give 280k reads/s, against 175k. Sync p99 is about 13 ms, the same as 2.6.4, since it is fsync-bound. 4.x drops `Durability::Eventual`, adds multi-process `ConcurrencyMode::MultiWriter`, and needs Rust 1.90+, which is above the project's 1.85 MSRV. Recommendation: plan an MSRV bump to 1.90 and a redb 4.x upgrade, with golden-file tests (ADR 0018), before the format freeze |

**Recommendation: keep redb as the engine (ADR 0002 confirmed). Ship 2.6.4 only if the MSRV must stay at 1.85; redb 4.6 removes most of the weak spots below. The spike also changes four spec items.**

1. Group commit must bound runs of `None` commits by forcing a durable commit. The harness used 1 per 100 writes.
2. On-demand ordering builds must commit `Immediate` per batch and use large, key-ordered batches.
3. The relaxed contract must say that relaxed data is lost on an app crash, not only on an OS crash.
4. Savepoint-based per-request rollback is impossible, so ADR 0025 overlays are required.

redb's weak spots, all within the gate:

- relaxed small-write latency, about 1.6x libmdbx;
- reader scaling: at 16 readers, 175k reads/s against libmdbx's 912k;
- bulk load, 2.4x slower than libmdbx.

These are worth tracking against redb upgrades; the redb 4.x row in the table above has the comparison.

Neither alternative is a safer choice than redb:

- **libmdbx** had the best relaxed latency and reader throughput. Through the `libmdbx` 0.9 Rust crate, though, it returned `MDBX_CORRUPTED` or `MDBX_BAD_TXN` to read transactions under 1 writer and 16 readers. It happened in most 10-second runs with 16 readers, at about 1 per 10M reads. Its sync cost equals redb's only in its own `DURABLE` mode, where `NoFlush` commits are no longer available per transaction.
- **Sanakirja 1.4.3** needed a locking fix before it could run concurrent readers at all. Every write transaction re-reads the whole free list, and `Flush` msyncs the entire map. The result is 2 ms relaxed writes, 35 ms sync writes, and a file 1.8x larger by length.

## Setup

| Item | Value |
| --- | --- |
| Machine | Apple M2 Max, 12 cores, 64 GB RAM |
| OS | macOS 27.2.1 (26B131), Darwin 27.2.0 |
| File system | APFS, internal SSD (`/System/Volumes/Data`, journaled), ~213 GB free at start |
| Rust | rustc 1.95.0, `cargo build --release`, `lto = "fat"`, `codegen-units = 1` |
| graphlava commit | d1bf69f plus the uncommitted working tree |
| Dataset | `gen-v1` (`src/data.rs`), seed 42 |
| Cache state | warm. `purge` needs root, so the OS page cache was never dropped |
| Process priority | benchmark processes ran at nice 5 (zsh `BG_NICE` on background jobs); all engines alike |
| Other load | an endpoint-security daemon (Avast) was active, and Spotlight indexing was on for the volume. Neither could be turned off without root. This is the likely cause of the occasional slow run (see the min-max columns) |

Engines and versions:

| Engine | Version | Adapter notes |
| --- | --- | --- |
| redb | `=2.6.4` (shipping pin) | Fixed-width tables `&[u8;24] -> &[u8;8]`, `&[u8;8] -> &[u8;26]`, `&[u8;8] -> &[u8]`. Cache 1 GiB (redb default). `Flush` = `Durability::Immediate`, `NoFlush` = `Durability::None` |
| libmdbx | crate `libmdbx` 0.9.0, C libmdbx from `mdbx-sys` 14.4.0 | `NoWriteMap`, default page size (16 KiB on this machine). Env in `SAFE_NOSYNC`. `NoFlush` = plain commit. `Flush` = commit + `mdbx_env_sync_ex(force)`, which issues `fsync` plus `F_FULLFSYNC` |
| Sanakirja | 1.4.3, **vendored with two patches** | 4 KiB pages, `n_roots = 4`. Map semantics are `del(k)` + `put(k,v)`, because Sanakirja trees are multimaps. Patch 1: `commit()` can skip msync, so `NoFlush` exists. Patch 2: the stock 1.4.3 `commit()` unlocks the root RwLock and then `Drop` unlocks it again; with concurrent readers this deadlocked the writer, so the unlocks were left to `Drop` alone. `Flush` = stock commit (msync) + `F_FULLFSYNC` |
| redb (comparison only) | 4.6.0 | same adapter (one macro); needs Rust 1.90+, so it cannot ship under MSRV 1.85 |

## Layout and data (v1 encoding)

- `spo`, `pos`, `osp`: 24-byte big-endian keys `(a,b,c)` mapping to an 8-byte LE statement id. `stmt`: an 8-byte BE id mapping to `s,p,o` (24 bytes LE) plus 2-byte flags. `props`: an 8-byte BE node mapping to a record with a 4-byte header and 16 bytes per entry.
- IDs follow tg.md/02-id-encoding.md. IRIs are kind `0x1`, sequential. Statements are kind `0x4`, chunked `(chunk+1)<<20 | offset`. Literals are inline integers `0xC<<60 | (2^57+v)`.
- 10M node-valued statements. There are 1.25M subject nodes with out-degree uniform 4..12. 64 predicates are skewed `floor(64u^2)`. Objects are 50% uniform and 50% hub-skewed `floor(nodes*u^3)`. Every node also has 4 inline integer properties in its `props` record (68 B).
- Bulk load: batches of 250k statements, each sorted per table, one write transaction per batch, committed with `Flush`. Batches committed with `NoFlush` make redb grow without bound; see the findings.

## Agent-memory mix (defined here; the 17-benchmarks mix needs a query layer)

| Class | % | Transaction |
| --- | --- | --- |
| `stmt_get` | 20 | read txn: `stmt` get by a random statement id |
| `props_get` | 20 | read txn: `props` get of a random node |
| `hop1_scan` | 10 | read txn: `spo` prefix `s` (out) or `osp` prefix `o` (in), 50/50, about 8 rows |
| `hop2_scan` | 10 | read txn: `spo(s)`, then `spo(o)` for each of up to 16 objects, about 70 rows |
| `prop_overwrite` | 15 | write txn: read the `props` record, change one value, write it back |
| `obj_overwrite` | 10 | write txn: find a live statement, 3 deletes and 3 inserts in the orderings, rewrite `stmt` |
| `insert_1_5` | 10 | write txn: 1-5 new statements (3 orderings + `stmt` each) |
| `delete` | 5 | write txn: find a live statement, delete it from 3 orderings and `stmt` |

Every write is its own small transaction. Latency is closed-loop on a single thread, measured from `begin_*` to commit or return, and recorded in HDR histograms (3 significant digits). The raw histograms are stored base64-encoded (V2+deflate) in the JSON. This is closed-loop, not the open-loop fixed-rate method that 17-benchmarks asks for, so coordinated omission is possible. All engines are equally affected, and the p99 comparison stands.

Relaxed (`NoFlush`) runs issue an untimed empty `Flush` commit after every 100th write (`--checkpoint 100`), like a background checkpointer. Without it, redb never frees copy-on-write pages, because pages are freed only by durable commits.

Runs: the latency mix used 5 runs per durability: 200k ops after 20k warm-up for relaxed, and 20k ops after 1k warm-up for sync. Sync is limited by `F_FULLFSYNC` at about 5 ms per commit, so 20k ops is below the spec's 10^5. Bulk loads ran once per engine. Throughput used 1 run of 10 s per configuration. The overwrite phase ran once. Reported values are the median run, with the min-max across runs.

Write amplification: bytes from `proc_pid_rusage(RUSAGE_INFO_V2).ri_diskio_byteswritten` for the process. It was checked against a known 100 MB write. APFS counts I/O in 16 KiB units, so one 4 KiB page write counts as 16 KiB. The logical bytes are key plus value of every insert and the key of every remove. Relaxed runs end with a `Flush` commit, so the counter sees the deferred writes.

## A. Engine bake-off at 10M statements

### Gate numbers

The ratio column is the other engine's advantage over redb. Above 2x fails redb.

| Metric | redb 2.6.4 | libmdbx | Sanakirja 1.4.3 | Best other vs redb |
| --- | --- | --- | --- | --- |
| Mix p99, relaxed (median of 5 runs; two independent 5-run sets each) | **453 µs** (rerun: 440-462) / 613 µs (first set: 494-8016, 2 disturbed runs) | 292 µs (rerun: 269-315) / 288 µs (276-320) | 2,138 µs | libmdbx **1.55x** (stable sets); 2.1x if redb's disturbed set is used |
| Mix p99, sync | 8.0 ms (first set) / 12.9 ms (rerun; bimodal per run, 7.9-14.1) | 20.8 / 22.7 ms with commit + `env_sync`; **7.8 ms** in libmdbx's own `DURABLE` mode | 34.9 ms | at most 1.0-1.6x, depending on which redb set |
| File after load | **411 B/stmt** (4.11 GB) | 417 B/stmt (16 KiB pages) | 725 B/stmt by length, 1,396 allocated | none better |
| File after 10^6 overwrites | 396 B/stmt (unchanged) | 416 B/stmt | 725 / 1,346 B/stmt | none better |

**redb cache size.** A larger cache does not change this. With an 8 GiB cache (the file is 4.1 GB), reads got faster: `stmt_get` p50 went from 3.8 to 2.4 µs. Writes got slower: write p50 rose from 175 to 212 µs. Relaxed mix p99 was 494 µs (486-501), and sync p99 8.9 ms.

The p99 gate passes. The closest call is relaxed p99 against libmdbx, at 1.55x across two stable 5-run sets. With the first, disturbed redb set it comes out 2.1x. Two of those runs had p99 of 2 ms and 8 ms, with wall time 2-3x longer, at the same time as the other engines' disturbed runs. A rerun of the same binary and data gave five runs within 440-462 µs, so the disturbed set is treated as noise. Rerun the latency mix on an idle machine before any decision rests on this margin.

### Bulk load, size, write amplification

| | redb | libmdbx | Sanakirja |
| --- | --- | --- | --- |
| Bulk load 10M (40 batches of 250k, each Flush) | 128.8 s (77.6k stmt/s) | 53.9 s (185k stmt/s) | 44.0 s (227k stmt/s) |
| Disk written during load | 23.9 GB | 33.1 GB | 24.6 GB |
| Disk bytes per small write txn, relaxed mix (incl. 1/100 checkpoint) | 349 KB | 294 KB | 189 KB |
| Disk bytes per small write txn, sync mix | 332 KB | 285 KB | 377 KB |
| 10^6 overwrites, 1 per txn, Flush every 100th: wall / disk bytes per overwrite | 671 s / 273 KB | 625 s / 241 KB | 2,470 s / 148 KB |
| Growth from 100k `NoFlush`-only overwrites | **+5.9 GB** (4.1 to 10.0 GB alloc; not returned after the next Immediate) | +0.1 GB | 0 |

About 190 logical key+value bytes change per small write. Every engine writes about 15-25 whole pages per small transaction, because each copy-on-write path touches 4 trees and the APFS counter counts in 16 KiB units. That makes write amplification about 1,000-2,000x per small transaction for all three engines. Batching is the only lever that matters; see C, bytes per request.

### Latency per operation class (median of runs; p50 / p99 / p99.9 µs)

Relaxed (redb from the stable rerun, libmdbx from the first set):

| Class | redb | libmdbx | Sanakirja |
| --- | --- | --- | --- |
| `stmt_get` | 3.8 / 11.8 / 25 | 3.4 / 10.1 / 23 | 2.0 / 5.6 / 8 |
| `props_get` | 3.3 / 11.0 / 23 | 2.8 / 9.7 / 22 | 2.6 / 6.3 / 9 |
| `hop1_scan` | 4.8 / 14.3 / 30 | 4.6 / 13.7 / 34 | 2.3 / 6.2 / 10 |
| `hop2_scan` | 33 / 66 / 112 | 27 / 57 / 106 | 13 / 22 / 29 |
| `prop_overwrite` | 70 / 153 / 314 | 56 / 172 / 476 | 1,826 / 2,337 / 2,988 |
| `obj_overwrite` | 207 / 400 / 843 | 132 / 340 / 836 | 1,866 / 2,384 / 2,957 |
| `insert_1_5` | 305 / 632 / 1,258 | 191 / 457 / 1,038 | 1,890 / 2,392 / 3,088 |
| `delete` | 165 / 331 / 633 | 109 / 288 / 773 | 1,850 / 2,355 / 2,789 |

Notes on reading this table:

- Reads are all single-digit microseconds. Sanakirja is fastest on reads, as a direct mmap B-tree with no copy.
- redb's small writes are about 1.2-1.6x libmdbx's at p50 and 0.9-1.4x at p99.
- Sanakirja writes cost about 1.8 ms each. `mut_txn_begin` re-reads the entire free-page B-tree into a `Vec` on every write transaction (profiled), so the cost grows with the free list.

In sync mode every write is one device flush. Write p50: redb 5.0 ms; libmdbx 9.0 ms with commit + `env_sync`, or 5.1 ms in `DURABLE` mode; Sanakirja 23 ms, because msync covers the whole 7.5 GB map and then `F_FULLFSYNC` runs.

### Throughput: 1 writer + N readers (ops/s, 10 s)

| Engine | Durability | Writer, 1 reader | Readers total, 1 / 4 / 16 | Writer, 16 readers |
| --- | --- | --- | --- | --- |
| redb | NoFlush | 1,220 | 121k / 221k / **175k** | 441 |
| redb | Flush | 212 | 147k / 366k / 223k | 168 |
| libmdbx | NoFlush | 1,680 | 243k / 667k / **912k** | 1,210 |
| libmdbx | Flush | 136 | 258k / 724k / 994k | 130 |
| Sanakirja | NoFlush | 379 | 311k / 611k / 452k | 75 |
| Sanakirja | Flush | 34 | 82k / 204k / 184k | 24 |

redb's reads do not scale past 4 threads on 12 cores: 16 readers give less total than 4. Its writer also slows as readers increase, from 1,220 to 441 writes/s. libmdbx scales almost linearly. This is not in the gate rule, but it matters for an IPC owner process serving many agents. The likely causes are the shared `TransactionTracker` and the cache locks in redb 2.x. redb 4.6.0 does better at 16 readers (280k reads/s, writer 1,130/s) but is still about 3x below libmdbx.

**libmdbx reader errors.** Through the `libmdbx` 0.9.0 crate, read transactions sometimes got `Err(Corrupted)`, and once `Err(BadTxn)`, while 1 writer and 16 readers ran.

| Run | Reader errors |
| --- | --- |
| First full run | aborted in the 16-reader phase |
| Dedicated check, shared DBI handles (3 x 10 s) | 3, 2 and 1 |
| Dedicated check, DBI handle opened per txn (3 x 10 s) | 1, 0 and 0 |
| Tail run (6 x 10 s) | 2 |

The rate is about 1 per 10 M reads. Smoke runs with 4 readers hit it once. Opening the DBI per transaction did not remove it, so the cause is the binding or libmdbx (with `MDBX_NOSTICKYTHREADS` and `SAFE_NOSYNC`), not the adapter's handle caching. It was not investigated further. redb and Sanakirja (after patch 2) returned no errors.

## B. Ordering count (redb only)

Only redb was run. libmdbx and Sanakirja were left out for time, and the question is about redb's layout anyway. Data is the same 10M dataset. The small-transaction write mix is 50% `insert_1_5` and 50% `obj_overwrite`. Relaxed: 5 runs x 50k ops. Sync: 3 runs x 3k ops.

| Metric | 3 orderings (SPO, POS, OSP) | 6 orderings | 3 vs 6 | ADR 0015 line |
| --- | --- | --- | --- | --- |
| Bulk load 10M | 113 s | 185 s | 39% faster | (n/a) |
| File after load (alloc) | 4.11 GB, 411 B/stmt | 6.72 GB, 672 B/stmt | **38.8% smaller** | at least 30% smaller: **met** |
| Disk bytes per small write txn | 514 KB | 869 KB | 41% less | (n/a) |
| Write p50 / p99, relaxed | 228 / 585 µs | 377 / 1,034 µs | **40% / 43% faster** | at least 30% faster: **met** |
| Write p50 / p99, sync | 5.88 / 8.13 ms | 6.02 / 9.20 ms | 2% / 12% faster | at least 30% faster: **not met** (`F_FULLFSYNC` dominates) |

The cyclic queries are p50 / p99 in µs, median of 5 runs. The anchored queries used 2,000 random anchors per run, and the unanchored query ran 5 times per run.

| Query | 3 orderings, 3-ordering plan | 3 + on-demand PSO/SOP/OPS (Flush-built), 6-ordering plan | 6 orderings, 3-ordering plan | 6 orderings, 6-ordering plan |
| --- | --- | --- | --- | --- |
| `tri1_anchored_p0`: (a p b)(b p c)(c p a), p = most frequent predicate | 7.6 / 19.3 | 6.7 / 17.3 | 8.2 / 20.3 | 7.6 / 20.7 |
| `cyc4_anchored_p0`: 4-cycle, same p | 9.3 / 33.7 | 8.2 / 24.9 | 9.1 / 30.9 | 9.0 / 26.9 |
| `tri_anypred_anchored`: (a ?p1 b)(b ?p2 c)(c ?p3 a) | 29.6 / 63.8 | 32.5 / 60.5 | 30.2 / 55.0 | 36.9 / 63.8 |
| `tri_all_rare_pred`: all triangles on a predicate with ~80k edges | 99.5 / 103.8 ms | 175 / 190 ms | 107 / 114 ms | 192 / 204 ms |

**Cyclic verdict: within 1.5x, and without any on-demand ordering.** The best plan on the 3-ordering file is at most 1.0x the best plan on the 6-ordering file on all four queries. The 3-ordering plans run as leapfrog-style sorted intersections over SPO and POS/OSP prefix scans. They work because the statement ID is the value and a bound predicate can sit in the middle of an SPO prefix: `(s,p,*)` is as good as `(p,s,*)`. The PSO/SOP/OPS plans were not faster. On `tri_all` they were 1.8x slower, because the PSO scan groups by subject, one group per edge for a rare predicate, where the POS scan groups by hub-skewed object, which gives fewer and larger groups. So the gain from extra orderings is a planner question, not an index question. Caveat: these are hand-written plans and the random graph has few triangles. A real WCO planner, and data with real clustering, could still find patterns that need a third ordering, such as variable predicates with a bound subject and a merge on the object.

**Online build of one ordering** (100k-entry batches; the build reads SPO and writes the target inside the same write transaction):

| Target | Batch commits | Time | File growth (alloc) | Disk written |
| --- | --- | --- | --- | --- |
| PSO | Flush | 13.9 s | +0 MB (filled free pages left by the load) | 2.2 GB |
| SOP | Flush | 12.6 s | +0 MB | 1.2 GB |
| OPS | Flush | 83.3 s | +721 MB | **39.6 GB** |
| OPS | NoFlush (first attempt) | 92.7 s | **+18.7 GB** | 1.4 GB |

After all three Flush-built orderings the file is 483 B/stmt, against 672 B/stmt for the always-six layout, because the new orderings fill free space. Two spec consequences:

1. Batches **must** commit durably. With `None`, redb freed nothing, and one random-order ordering grew the file from 4.1 GB to 22.8 GB.
2. The cost depends on key order. PSO and SOP keys arrive almost in SPO order, so batches append. OPS keys arrive in random order, so each 100k batch rewrites most of the tree: 39.6 GB written for a 333 MB index. Build with much larger batches, or sort-merge (external sort, then append in key order). `pl-ondemand-online` should name this.

## C. redb crash repair

Method:

- The base file is built once with `crash-prep`: N statements in 3 orderings, then a clean close.
- Each trial makes an APFS clone of the base (`cp -c`).
- A child process opens the clone, brings it into the mode's state, and prints `READY`. The parent then sends `SIGKILL`.
- A fresh process (`open-timer`) times `Builder::open`, including the repair that redb runs when the header has `recovery_required`. It also times a first `stmt` get. A second fresh process then reopens after the repaired file was closed cleanly.
- The OS page cache is warm, because the base file was just written or read. These are therefore lower bounds for a cold open.
- The modes:
  - `uncommitted_big`: 1M statements written in one write txn, no commit.
  - `after_noflush`: 200 small `None` commits.
  - `after_flush`: 200 `None` + 200 `Immediate` commits.
  - `qr_*`: every write txn has `set_quick_repair(true)`.

**10M statements (4.1 GB file), 3 reps each:**

| State at `kill -9` | Open time (median, min-max) | Repair ran |
| --- | --- | --- |
| clean close (baseline) | 0.005 s | no |
| big uncommitted txn, no commit since open | **3.4 s** (3.28-3.43) | yes |
| after 200 `None` commits | **15.5 s** (15.48-15.55) | yes |
| after `None` + `Immediate` commits | **15.5 s** (15.43-15.77) | yes |
| quick-repair on, after `Immediate` + `None` commits | **0.009 s** | no full repair |
| quick-repair on, big uncommitted txn | 0.083 s (0.020-0.089) | no full repair |

**100M statements (43 GB file, 429 B/stmt; loaded in 1,229 s with 5M-statement Flush batches), 1 rep each:**

| State at `kill -9` | Open time | Repair ran |
| --- | --- | --- |
| clean close | 0.007 s | no |
| big uncommitted txn (100k stmts), no commit since open | **202 s** (an interrupted first attempt gave 145 s) | yes |
| after 200 `None` commits | **105 s** | yes |
| after `None` + `Immediate` commits | **221 s** | yes |
| quick-repair on, after `Immediate` + `None` commits | **0.012 s** | no full repair |

At 100M the trial clones are only partly in the page cache: 43 GB file, 64 GB RAM, and each APFS clone is a new vnode. So the 100M numbers sit between warm and cold. Repair scales roughly linearly to worse with file size: 15.5 s at 4 GB and 105-221 s at 43 GB, about 2.5-5 s per GB. With quick-repair, small `Immediate` txns at 100M had p50 28 ms against 11 ms without.

Findings:

- **Any unclean shutdown after a write repairs the whole file**, and the time grows with file size. redb sets `recovery_required` in the header when the database is opened writable (`page_manager.rs:371-377` `begin_writable`) and clears it only on a clean drop. A durable commit right before the kill does not help: 15.5 s at 10M either way.
- **Repair holds memory.** RSS after the repaired open is about 1.2 GB, against 4 MB for a clean open.
- **`set_quick_repair(true)` cuts the repaired open to about 10 ms**, at the cost of slower durable commits. Small `Immediate` txns had p50 15.0 ms with quick-repair against 8.0-8.9 ms without at 10M, and 28 ms against 11 ms at 100M, measured in a freshly opened child process. Quick-repair turns on 2-phase commit (2 syncs) and saves the allocator state on every durable commit. `None` commits were not slower (p50 4.3-4.9 ms against 5.1-5.7 ms; these first-after-open txns are dominated by a cold redb cache).
- For graphlava this answers [[open-questions#Crash repair time on large files]]. Without quick-repair, a 10M-statement file costs about 15 s of app start after every unclean kill; about 2-4 minutes at 100M. On iOS, where OS kills are unclean, that is unacceptable. **Recommendation: enable quick-repair on every durable commit (or on the group-commit batch), and accept about 2x durable-commit latency.** Alternatively, make group commit amortise it, since one durable commit per batch pays the cost once. 06-transactions-durability "Crash recovery" and 07-platforms iOS should state this.

**Bytes written per one-statement request** (ADR 0025, roadmap first-week fact; 10M file, 2,000 requests per mode, 1 statement = 3 orderings + `stmt`):

| Engine transactions | Disk bytes per request | Requests/s |
| --- | --- | --- |
| one `Immediate` txn per request | 303 KB | 163 |
| one `Immediate` txn per 100-request batch | 110 KB | 951 |
| one `None` txn per request, `Immediate` every 100 | 311 KB | 681 |

Batching 100 requests in one transaction writes 2.7x fewer bytes per request and is 5.8x faster. One `None` transaction per request saves sync time but not bytes, because every commit still writes its own copy-on-write path. The batch transaction of ADR 0025 is what reduces I/O, which supports group commit with per-request overlays over per-request engine transactions. Only 2.7x and not 100x, because random keys in `pos`/`osp` still touch about 100 distinct leaf pages per batch.

## D. Durability mapping in redb 2.6.4

**Answer: yes.** An `Immediate` commit makes every earlier `None` commit durable. Two caveats matter for the design:

1. A `None` commit does not survive a *process* crash. Losing power is not required.
2. `None` commits never free pages, so a stream of them grows the file without bound.

Evidence from the empirical test (`dur-test`, `results/D-durability.json`): a child process commits, gets `kill -9`, and the file is then reopened. 3 reps per scenario:

| Scenario (after a `base` Immediate commit) | Present after reopen (3 of 3 reps) |
| --- | --- |
| `A` None, `B` Immediate, `C` None, kill | `A`, `B` (the None commit before the Immediate survived; the trailing `C` is lost) |
| `A` None, kill | none (lost on a plain process kill, while the OS page cache was intact) |
| `A` Eventual, kill | `A` |
| 100 None, then `B` Immediate, kill | all 100 plus `B` |

Source, redb 2.6.4 in `~/.cargo/registry/src/*/redb-2.6.4/src`:

- `transactions.rs:356-373`: `Durability::None` is documented as "will not be persisted to disk unless followed by a commit with a higher durability level", and "Pages are only freed during commits with higher durability levels. Exclusively using this durability level will result in rapid growth of the database file."
- `transactions.rs:1776`: `InternalDurability::None => self.non_durable_commit(...)`. Immediate goes to `durable_commit(user_root, false)` and Eventual to `durable_commit(user_root, true)`.
- `tree_store/page_store/page_manager.rs:804-831` `non_durable_commit`: calls `storage.write_barrier()` (`cached_file.rs:303`, which only drains redb's write buffer to the file and does not sync). It then updates **only the in-memory** secondary header slot (`read_from_secondary = true`), and nothing is written to the on-disk header. That is why a process kill loses it.
- `page_manager.rs:731-800` `commit_inner` (durable): writes the header slot that points at the current tree roots. Those roots already include every prior non-durable commit, because the B-trees are copy-on-write and cumulative. It then calls `storage.flush(eventual)` (lines 766/776). `cached_file.rs:296-300` `flush` drains the write buffer and calls `file.sync_data(eventual)`. Every earlier `None` commit's pages were already handed to the OS by `write_barrier`, so this one sync makes them durable. `transactions.rs:2018` then calls `clear_pending_non_durable_commits()`.
- Default is 1-phase commit with checksums (`set_two_phase_commit` docs, `transactions.rs:1575-1616`): one sync per Immediate commit. 2-phase commit adds a second sync and is turned on automatically by quick-repair.

**Sync call on macOS** (`tree_store/page_store/file_backend/unix.rs:66-83`): for `Immediate`, `sync_data(false)` calls `self.file.sync_data()`. Rust std (1.95, `library/std/src/sys/fs/unix.rs:1395-1402`, `os_datasync`) implements that on Apple targets as `fcntl(fd, F_FULLFSYNC)`. For `Eventual`, `sync_data(true)` calls `fcntl(fd, F_BARRIERFSYNC)` directly. So on Apple, Immediate is a full device-cache flush, and Eventual is a barrier: ordered, not flushed. Measured cost of an Immediate small transaction on this SSD was about 5 ms p50 (sync mix writes).

What this means for 05-storage-trait `st-durability-map` and 06 "Durability levels":

- `Flush -> Immediate` is correct, and `F_FULLFSYNC` is what the spec wanted.
- `NoFlush -> None` meets "becomes durable no later than the next Flush commit". But a `relaxed` batch is lost even on an app crash with no OS crash. Phrase the relaxed contract that way. On iOS, where every kill by the OS is unclean, relaxed data that is not followed by an ordered or sync batch is always lost.
- `None` alone grows the file without bound. This is measured, not just documented. 100k single-overwrite `None` commits grew the 10M file from 4.1 GB to 10.0 GB allocated. It did **not** shrink after the next Immediate commit; the space became free inside the file. Earlier, 1M statements bulk-loaded in 4 `None` batches made a 953 MB file, against about 400 MB with Immediate batches. The group-commit design must therefore force an `Immediate` (or `Eventual`) commit every N `None` commits or M MB, even for an all-`relaxed` stream. The harness used one empty `Flush` per 100 writes.
- `Durability::Eventual` (F_BARRIERFSYNC) is a candidate for `ordered` on Apple: it orders writes and costs much less than F_FULLFSYNC. It keeps data across a process crash (tested). Across power loss it keeps ordering but not recency. redb 3.x/4.x removed `Eventual`; 4.6.0 has only `None` and `Immediate`.

## Savepoint after write (redb 2.6.4)

**Answer: not allowed.** Both savepoint kinds fail with `SavepointError::InvalidSavepoint`, Display `"Savepoint is invalid or cannot be created."`, as soon as the transaction is "dirty", and **merely opening a table makes it dirty**. Empirical results (`savepoint-test`, `results/C-savepoint.json`):

| Call | Result |
| --- | --- |
| `ephemeral_savepoint()` on a fresh write txn | Ok |
| `ephemeral_savepoint()` after `open_table` (no write) | `Err(InvalidSavepoint)` |
| `ephemeral_savepoint()` after an insert | `Err(InvalidSavepoint)` |
| `persistent_savepoint()` after an insert | `Err(InvalidSavepoint)` |
| `persistent_savepoint()` on a fresh txn with `Durability::None` | `Err(InvalidSavepoint)` (requires Immediate) |
| savepoint taken first, insert, `restore_savepoint()` in the same txn | Ok, and the insert is gone |
| `ephemeral_savepoint()` after that restore | `Err(InvalidSavepoint)` |

Source: `transactions.rs:1176-1178` (`if self.dirty.load(..) { return Err(SavepointError::InvalidSavepoint) }`), `transactions.rs:607-613` (`set_dirty`, called when a user table is opened, `:649`/`:671`), `transactions.rs:1060-1062` (persistent savepoints need Immediate).

Consequence: a per-request rollback point inside one batch transaction cannot be built from redb savepoints, except for the first request of the batch. The ADR 0025 approach, per-request overlays in the batch transaction, is required. A savepoint at the start of the batch can still serve as a "rollback whole batch" point, but `abort()` does the same.

## What was not run, and deviations

- **100M bake-off was not run for any engine.** It was optional, and each 10M engine run took 30-75 minutes. A 100M redb file was built only for the crash-repair test; its load time is in `results/C-prep-100000000.json`.
- **B (ordering count) ran on redb only.** It was not run on libmdbx or Sanakirja, and not at 100M.
- **The 17-benchmarks agent-memory mix was not implemented.** That mix needs graph scopes, star patterns and annotation writes. A storage-level mix was used instead (see Setup). The latency method is closed-loop, not open-loop at a fixed rate.
- **Run counts were below the 5-run rule in places.**
  - 5 runs for each latency mix; redb and libmdbx latency phases were later run a second time.
  - 1 run for each bulk load, throughput point, overwrite phase and online build.
  - Sync mixes have 20k ops per run, not 10^5.
  - No bootstrap confidence intervals: the median and min-max of runs are reported instead.
- **The environment was not quiet.** Caches were warm (no root for `purge`). Processes ran at nice 5. An antivirus daemon and Spotlight were active. Some runs were disturbed, as the min-max columns show. Treat single-digit-percent differences as noise.
- **libmdbx caveats.**
  - The `relaxed`/`sync` mapping is commit + `env_sync`. A separate run in the engine's own `DURABLE` mode gave its true sync cost.
  - The first full libmdbx run aborted on a reader `MDBX_CORRUPTED`. Its latency phases were kept. Throughput, overwrites and the `NoFlush` probe come from a second "tail" run with a fresh load; `merge_mdbx.py` combines them.
  - libmdbx used its default 16 KiB pages and redb 4 KiB. Page size was not swept (ADR 0035).
- **Sanakirja needed two vendored patches to run at all.** Patch 1 lets `commit()` skip msync for `NoFlush`. Patch 2 fixes the double unlock in `commit()`. Its allocated size includes `F_PREALLOCATE` space beyond EOF, so both length and allocated size are reported. Sanakirja 2.0 beta was not tried; it has the same double unlock in `commit()` (`environment/muttxn.rs:615` and `Drop` at `:137-145`).
- **Partial coverage of the layout and tests.**
  - The `plist`, `*_m` multi tables and value index were not in the layout.
  - No fault-injection or power-cut test was run. D is a process-kill test plus a source reading.
- **Crash repair was measured only on the desktop and warm.** No phone run, so the 17-benchmarks "Open and recovery time" target on mobile is still open.
- **The 100M crash test has 1 rep per mode, not 3.** At 100M the child that builds the uncommitted transaction was cut to 100k statements, because 1M random inserts into a cold 43 GB clone took more than 15 minutes. One extra `uncommitted_big` rep from an interrupted first attempt is in `logs/C-run-100M-attempt1.log`. The 100M base was loaded with 5M-statement batches and an 8 GiB cache. With 250k-statement batches the load was super-linear (24M statements after 545 s and slowing). Each Flush batch rewrites most leaves of the random-order `pos`/`osp` trees, so bulk load needs large or sorted-run batches.
- **An outside process interfered once.** Between 17:15 and 17:35 a duplicate driver from the parent session re-ran crash-prep and crash-run on the same files. All C10 numbers reported here come from a clean rerun after that (17:30-17:39).

## Reproduce

```sh
cd spikes/bakeoff
cargo build --release          # needs Xcode SDK for libmdbx bindgen; see .cargo/config.toml
B=./target/release/bakeoff
$B smoke                                         # all adapters, correctness checks, plans agree
$B bake --engine redb|mdbx|sana|redb4 --n 10000000   # A (phases: --phases lat,tp,ow,probe | latr | lats)
$B orders --engine redb --layout 3|6 --n 10000000    # B (--skip_writes 1 --build_dur flush|noflush)
$B crash-prep --n 10000000 --db data/crash/base-10M.redb && $B crash-run --base data/crash/base-10M.redb   # C
$B savepoint-test; $B dur-test                   # savepoints, D
$B tpcheck --engine mdbx --readers 16            # libmdbx reader-error check
python3 merge_mdbx.py; python3 summarize.py      # tables
```

`run_all.sh` and `run_q*.sh` are the sequential drivers that were actually used. Data goes to `data/` (gitignored), and each run deletes its data when it finishes.

## Appendix: generated tables

### A. Bulk load and file size (10M)

| engine | load s | stmts/s | MB after load | B/stmt after load (alloc) | B/stmt before overwrites | B/stmt after 10^6 overwrites | NoFlush-only probe growth MB |
|---|---|---|---|---|---|---|---|
| redb-2.6.4 | 128.8 | 77,643 | 4,109 | 410.9 | 395.6 | 395.6 | 5,931 |
| libmdbx-0.9.0 | 53.9 | 185,376 | 4,172 | 417.2 | 415.7 | 415.7 | 123 |
| sanakirja-1.4.3 | 44.0 | 227,033 | 13,959 | 1,395.9 | 1,346.2 | 1,346.2 | 0 |

### A. Mix latency, relaxed (µs; median of runs, [min-max of runs] for p99)

| engine | runs x ops | mix p50 | mix p99 | mix p99 range | mix p99.9 | writes p50 | writes p99 | ops/s |
|---|---|---|---|---|---|---|---|---|
| redb-2.6.4 | 5 x 200000 | 18.9 | 612.9 | 494-8016 | 1,470.5 | 199.6 | 798.7 | 2,500 |
| libmdbx-0.9.0 | 5 x 200000 | 13.5 | 288.3 | 276-320 | 575.0 | 115.8 | 350.0 | 3,595 |
| sanakirja-1.4.3 | 5 x 200000 | 6.9 | 2,138.1 | 2126-2185 | 2,713.6 | 1,854.5 | 2,363.4 | 995 |

Per class p50 / p99 / p99.9 (µs), relaxed:

| class | redb-2.6.4 | libmdbx-0.9.0 | sanakirja-1.4.3 |
|---|---|---|---|
| delete | 183.2 / 628.2 / 2,324.5 | 109.2 / 288.3 / 772.6 | 1,850.4 / 2,355.2 / 2,789.4 |
| hop1_scan | 5.5 / 18.7 / 71.4 | 4.6 / 13.7 / 33.8 | 2.3 / 6.2 / 10.2 |
| hop2_scan | 36.1 / 87.0 / 352.8 | 26.5 / 57.1 / 105.9 | 12.8 / 21.8 / 29.3 |
| insert_1_5 | 360.7 / 1,147.9 / 2,869.2 | 190.5 / 456.7 / 1,038.3 | 1,890.3 / 2,392.1 / 3,088.4 |
| obj_overwrite | 231.6 / 712.2 / 2,277.4 | 132.0 / 340.2 / 836.1 | 1,865.7 / 2,383.9 / 2,957.3 |
| prop_overwrite | 77.2 / 247.7 / 1,028.6 | 56.1 / 172.2 / 476.4 | 1,825.8 / 2,336.8 / 2,988.0 |
| props_get | 3.6 / 13.0 / 41.4 | 2.8 / 9.7 / 21.6 | 2.6 / 6.3 / 8.8 |
| stmt_get | 4.2 / 14.7 / 68.9 | 3.4 / 10.1 / 22.8 | 2.0 / 5.6 / 8.2 |

Write I/O during the relaxed mix (median of runs): disk bytes per write op / bytes per kv byte changed

| engine | disk B per write op | disk B / kv B |
|---|---|---|
| redb-2.6.4 | 349,068 | 1,871.7 |
| libmdbx-0.9.0 | 293,533 | 1,571.4 |
| sanakirja-1.4.3 | 188,547 | 1,008.3 |

### A. Mix latency, sync (µs; median of runs, [min-max of runs] for p99)

| engine | runs x ops | mix p50 | mix p99 | mix p99 range | mix p99.9 | writes p50 | writes p99 | ops/s |
|---|---|---|---|---|---|---|---|---|
| redb-2.6.4 | 5 x 20000 | 44.4 | 7,962.6 | 7840-15589 | 10,076.2 | 5,087.2 | 8,888.3 | 455 |
| libmdbx-0.9.0 | 5 x 20000 | 31.9 | 20,758.5 | 12255-34341 | 40,206.3 | 8,994.8 | 23,167.0 | 264 |
| sanakirja-1.4.3 | 5 x 20000 | 9.7 | 34,897.9 | 34079-35684 | 38,993.9 | 23,183.4 | 36,864.0 | 102 |

Per class p50 / p99 / p99.9 (µs), sync:

| class | redb-2.6.4 | libmdbx-0.9.0 | sanakirja-1.4.3 |
|---|---|---|---|
| delete | 5,140.5 / 7,876.6 / 10,780.7 | 9,011.2 / 27,836.4 / 46,334.0 | 23,101.4 / 36,470.8 / 38,993.9 |
| hop1_scan | 14.9 / 72.1 / 187.5 | 12.2 / 55.4 / 235.5 | 4.6 / 11.1 / 21.9 |
| hop2_scan | 82.2 / 287.5 / 933.9 | 54.0 / 190.5 / 296.4 | 17.4 / 30.0 / 58.1 |
| insert_1_5 | 6,025.2 / 9,961.5 / 12,312.6 | 9,936.9 / 26,001.4 / 43,876.4 | 24,002.6 / 37,224.4 / 40,009.7 |
| obj_overwrite | 5,853.2 / 8,081.4 / 11,894.8 | 9,068.5 / 31,719.4 / 59,146.2 | 23,806.0 / 37,027.8 / 40,927.2 |
| prop_overwrite | 4,870.1 / 6,881.3 / 9,650.2 | 7,884.8 / 19,038.2 / 34,930.7 | 23,019.5 / 36,044.8 / 39,223.3 |
| props_get | 8.8 / 56.6 / 199.7 | 5.5 / 32.0 / 82.1 | 3.5 / 9.0 / 18.0 |
| stmt_get | 11.0 / 61.8 / 277.2 | 8.3 / 39.6 / 141.2 | 2.8 / 8.0 / 14.5 |

Write I/O during the sync mix (median of runs): disk bytes per write op / bytes per kv byte changed

| engine | disk B per write op | disk B / kv B |
|---|---|---|
| redb-2.6.4 | 331,559 | 1,776.3 |
| libmdbx-0.9.0 | 284,550 | 1,528.9 |
| sanakirja-1.4.3 | 376,954 | 2,030.1 |

### A. Throughput, 1 writer + N readers (ops/s, 10 s, 1 run)

| engine | durability | readers | writer ops/s | reader ops/s total | per reader thread | reader errors |
|---|---|---|---|---|---|---|
| redb-2.6.4 | NoFlush | 1 | 1,220 | 120,648 | 120,648 | n/a |
| redb-2.6.4 | NoFlush | 4 | 881 | 221,179 | 55,295 | n/a |
| redb-2.6.4 | NoFlush | 16 | 441 | 174,668 | 10,917 | n/a |
| redb-2.6.4 | Flush | 1 | 212 | 146,734 | 146,734 | n/a |
| redb-2.6.4 | Flush | 4 | 204 | 366,226 | 91,556 | n/a |
| redb-2.6.4 | Flush | 16 | 168 | 223,160 | 13,948 | n/a |
| libmdbx-0.9.0 | NoFlush | 1 | 1,680 | 242,629 | 242,629 | 0 |
| libmdbx-0.9.0 | NoFlush | 4 | 1,450 | 666,744 | 166,686 | 0 |
| libmdbx-0.9.0 | NoFlush | 16 | 1,210 | 912,430 | 57,027 | 2 |
| libmdbx-0.9.0 | Flush | 1 | 136 | 257,681 | 257,681 | 0 |
| libmdbx-0.9.0 | Flush | 4 | 135 | 724,406 | 181,102 | 0 |
| libmdbx-0.9.0 | Flush | 16 | 130 | 994,393 | 62,150 | 0 |
| sanakirja-1.4.3 | NoFlush | 1 | 379 | 310,670 | 310,670 | n/a |
| sanakirja-1.4.3 | NoFlush | 4 | 255 | 610,538 | 152,634 | n/a |
| sanakirja-1.4.3 | NoFlush | 16 | 75 | 451,774 | 28,236 | n/a |
| sanakirja-1.4.3 | Flush | 1 | 34 | 82,306 | 82,306 | n/a |
| sanakirja-1.4.3 | Flush | 4 | 32 | 204,387 | 51,097 | n/a |
| sanakirja-1.4.3 | Flush | 16 | 24 | 184,128 | 11,508 | n/a |

### A. 10^6 overwrites (1 per txn, NoFlush, every 100th Flush)

| engine | wall s | disk bytes/overwrite | disk B / kv B |
|---|---|---|---|
| redb-2.6.4 | 670.8 | 273,218 | 1,965.9 |
| libmdbx-0.9.0 | 624.5 | 240,720 | 1,732.0 |
| sanakirja-1.4.3 | 2,469.7 | 147,528 | 1,061.8 |

### A. Mix latency, all runs including reruns and variants (µs)

| file | engine | durability | runs | mix p50 | mix p99 (median) | p99 per run | mix p99.9 | writes p50 | writes p99 |
|---|---|---|---|---|---|---|---|---|---|
| A-bake-mdbx-10000000.json | libmdbx-0.9.0 (cache 0 MiB) | relaxed | 5 | 13.5 | 288.3 | 288, 320, 313, 276, 276 | 575.0 | 115.8 | 350.0 |
| A-bake-mdbx-10000000.json | libmdbx-0.9.0 (cache 0 MiB) | sync | 5 | 31.9 | 20,758.5 | 27001, 12255, 12976, 20759, 34341 | 40,206.3 | 8,994.8 | 23,167.0 |
| A-bake-mdbx-durableenv-10000000.json | libmdbx-0.9.0 (cache 0 MiB) | sync | 5 | 44.0 | 7,823.4 | 6996, 7614, 7823, 11264, 8495 | 10,944.5 | 5,070.8 | 8,863.7 |
| A-bake-mdbx-latrerun-10000000.json | libmdbx-0.9.0 (cache 0 MiB) | relaxed | 5 | 14.3 | 291.6 | 312, 277, 292, 315, 269 | 835.1 | 118.7 | 385.5 |
| A-bake-mdbx-latrerun-10000000.json | libmdbx-0.9.0 (cache 0 MiB) | sync | 5 | 31.3 | 22,659.1 | 25035, 12124, 21725, 22659, 24035 | 36,667.4 | 9,003.0 | 28,934.1 |
| A-bake-redb-10000000.json | redb-2.6.4 (cache 1024 MiB) | relaxed | 5 | 18.9 | 612.9 | 496, 2023, 8016, 494, 613 | 1,470.5 | 199.6 | 798.7 |
| A-bake-redb-10000000.json | redb-2.6.4 (cache 1024 MiB) | sync | 5 | 44.4 | 7,962.6 | 7840, 7963, 7963, 15589, 8053 | 10,076.2 | 5,087.2 | 8,888.3 |
| A-bake-redb-cache8g-10000000.json | redb-2.6.4 (cache 8192 MiB) | relaxed | 5 | 12.5 | 493.6 | 493, 501, 495, 494, 486 | 666.6 | 211.6 | 545.3 |
| A-bake-redb-cache8g-10000000.json | redb-2.6.4 (cache 8192 MiB) | sync | 5 | 38.0 | 8,929.3 | 8995, 8929, 8880, 8929, 16630 | 11,018.2 | 5,857.3 | 9,838.6 |
| A-bake-redb-latrerun-10000000.json | redb-2.6.4 (cache 1024 MiB) | relaxed | 5 | 16.2 | 452.6 | 462, 457, 453, 440, 450 | 688.1 | 175.1 | 512.3 |
| A-bake-redb-latrerun-10000000.json | redb-2.6.4 (cache 1024 MiB) | sync | 5 | 38.7 | 12,894.2 | 12894, 13738, 14057, 7930, 8065 | 15,818.8 | 5,021.7 | 13,983.7 |
| A-bake-redb4-10000000.json | redb-4.6.0 (cache 1024 MiB) | relaxed | 5 | 10.8 | 169.7 | 161, 172, 162, 170, 172 | 241.9 | 58.3 | 199.2 |
| A-bake-redb4-10000000.json | redb-4.6.0 (cache 1024 MiB) | sync | 5 | 21.6 | 13,058.0 | 7115, 13066, 12116, 13058, 20005 | 16,130.0 | 5,079.0 | 14,139.4 |
| A-bake-sana-10000000.json | sanakirja-1.4.3 (cache 0 MiB) | relaxed | 5 | 6.9 | 2,138.1 | 2185, 2152, 2134, 2138, 2126 | 2,713.6 | 1,854.5 | 2,363.4 |
| A-bake-sana-10000000.json | sanakirja-1.4.3 (cache 0 MiB) | sync | 5 | 9.7 | 34,897.9 | 34144, 34079, 34898, 35684, 34931 | 38,993.9 | 23,183.4 | 36,864.0 |

### B. Ordering count

| engine | layout | load s | B/stmt alloc | small-txn write p50 relaxed | p99 relaxed | p50 sync | p99 sync |
|---|---|---|---|---|---|---|---|
| redb-2.6.4 (B-orders-redb-3-10000000.json) | spo,pos,osp | 113.1 | 410.9 | 227.6 | 585.2 | 5,881.9 | 8,134.7 |
| redb-2.6.4 (B-orders-redb-3-flushbuild-10000000.json) | spo,pos,osp | 109.3 | 410.9 | nan | nan | nan | nan |
| redb-2.6.4 (B-orders-redb-6-10000000.json) | spo,pos,osp,pso,sop,ops | 185.1 | 671.8 | 377.3 | 1,033.7 | 6,021.1 | 9,199.6 |

Cyclic queries p50 / p99 (µs; median over runs):

| engine | layout | plan | tri1_anchored_p0 | cyc4_anchored_p0 | tri_anypred_anchored | tri_all_rare_pred |
|---|---|---|---|---|---|---|
| redb-2.6.4 redb-3 | 3 | Three | 7.3 / 19.4 | 8.8 / 30.4 | 28.8 / 51.8 | 104,464.4 / 110,100.5 |
| redb-2.6.4 redb-3 | 3+ondemand(pso,sop,ops) | Six | 7.7 / 32.0 | 8.6 / 38.3 | 38.6 / 110.0 | 200,802.3 / 231,473.2 |
| redb-2.6.4 redb-3-flushbuild | 3 | Three | 7.6 / 19.3 | 9.3 / 33.7 | 29.6 / 63.8 | 99,549.2 / 103,809.0 |
| redb-2.6.4 redb-3-flushbuild | 3+ondemand(pso,sop,ops) | Six | 6.7 / 17.3 | 8.2 / 24.9 | 32.5 / 60.5 | 175,112.2 / 190,316.5 |
| redb-2.6.4 redb-6 | 6 | Three | 8.2 / 20.3 | 9.1 / 30.9 | 30.2 / 55.0 | 106,758.1 / 114,098.2 |
| redb-2.6.4 redb-6 | 6 | Six | 7.6 / 20.7 | 9.0 / 26.9 | 36.9 / 63.8 | 192,282.6 / 204,341.2 |

Online builds (B-orders-redb-3-10000000.json): pso (NoFlush batches of 100000): 12.9 s, +0 MB alloc, 1,487 MB written; sop (NoFlush batches of 100000): 11.7 s, +0 MB alloc, 456 MB written; ops (NoFlush batches of 100000): 92.7 s, +18,666 MB alloc, 1,369 MB written; B/stmt 410.9 -> 2,277.5

Online builds (B-orders-redb-3-flushbuild-10000000.json): pso (Flush batches of 100000): 13.9 s, +0 MB alloc, 2,164 MB written; sop (Flush batches of 100000): 12.6 s, +0 MB alloc, 1,248 MB written; ops (Flush batches of 100000): 83.3 s, +721 MB alloc, 39,557 MB written; B/stmt 410.9 -> 483.0

### C. Crash repair, results/C-crash-100M.json (base 42.52 GB, 100,000,000 stmts)

| mode | reps | open s (median) | min-max | repaired (callback calls>0) | first query ms | clean reopen s |
|---|---|---|---|---|---|---|
| clean | 1 | 0.007 | 0.007-0.007 | 0/1 | 0.61 | 0.006 |
| after_flush | 1 | 220.907 | 220.907-220.907 | 1/1 | 0.71 | 0.011 |
| qr_after_noflush | 1 | 0.012 | 0.012-0.012 | 0/1 | 0.54 | 0.011 |
| uncommitted_big | 1 | 201.705 | 201.705-201.705 | 1/1 | 0.23 | 0.006 |
| after_noflush | 1 | 104.975 | 104.975-104.975 | 1/1 | 0.02 | 0.007 |

### C. Crash repair, results/C-crash-10M.json (base 4.11 GB, 10,000,000 stmts)

| mode | reps | open s (median) | min-max | repaired (callback calls>0) | first query ms | clean reopen s |
|---|---|---|---|---|---|---|
| clean | 3 | 0.005 | 0.005-0.005 | 0/3 | 0.48 | 0.005 |
| uncommitted_big | 3 | 3.414 | 3.280-3.426 | 3/3 | 0.02 | 0.005 |
| after_noflush | 3 | 15.508 | 15.476-15.545 | 3/3 | 0.02 | 0.005 |
| after_flush | 3 | 15.469 | 15.431-15.765 | 3/3 | 0.02 | 0.005 |
| qr_after_noflush | 3 | 0.009 | 0.008-0.010 | 0/3 | 0.49 | 0.005 |
| qr_uncommitted_big | 3 | 0.083 | 0.020-0.089 | 0/3 | 0.48 | 0.005 |
