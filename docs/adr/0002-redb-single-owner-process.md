---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Use redb as the storage engine with a single owner process

## Context and Problem Statement

meta-core needs a ready-made transactional KV engine behind a `Storage` trait (handover D14). The choice depends on whether two OS processes must open the same database file at once. iOS kills processes that hold locks on app-group files when suspended (`0xdead10cc`), and libmdbx multi-process mode needs a `-lck` file with shared-memory reader tables.

## Considered Options

<option id="redb" outcome="chosen">

redb: pure Rust, single file, easy iOS/Android/Windows builds, multimap tables, savepoints, already used by tiramemsu. One process owns the file.

</option>

<option id="libmdbx" outcome="rejected">

libmdbx: multi-process access, but a second lock file, C toolchain on every platform (bindgen/libclang), vendored source, and lock handling that conflicts with iOS suspension.

</option>

<option id="sanakirja" outcome="deferred">

Sanakirja: O(1) database clone is attractive for what-if branches, but fixed 4 KB pages and a small community. Revisit only if clones become a requirement.

</option>

## Decision Outcome

Chosen option: "redb", because multi-process file access is not a requirement. The contract is **one owner process per database file**; other processes reach it through IPC (XPC / local socket / daemon on desktop).

### Consequences

<consequence kind="good">

Pure-Rust build on all five target platforms; no lock-file or `0xdead10cc` handling in the engine layer; process-local caches (pages, plan cache, stats) are safe.

</consequence>

<consequence kind="bad">

App extensions and CLI-plus-app setups need an IPC layer. redb has no native page-level compression or 16 KB page option, so the v2 block encoding must be built in the value layer. The `Storage` trait stays, so an own pager remains possible.

</consequence>

## More Information

Resolves handover open question 2. The bake-off in `docs/17-benchmarks.md` still validates redb against the real key layout. Note: user wrote "reddb"; interpreted as redb (cberner/redb).
