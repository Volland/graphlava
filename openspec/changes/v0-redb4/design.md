# Design

## Context

ADR 0002 picked redb; the bake-off confirmed it at 2.6.4 and measured 4.6.0 beside it. [[22-implementation-decisions#redb 2.6.4 with file format v3]] pinned 2.6.4 only because of the MSRV.

## Decisions

| Decision | Source |
| --- | --- |
| Upgrade to redb 4.6.x before the format freeze | bake-off RESULTS.md, "redb 4.x" |
| MSRV 1.90 | redb 4.x `rust-version` |
| `ordered` and `relaxed` keep their contracts without `Durability::Eventual` | [[06-transactions-durability#Durability levels]] |
| Quick repair stays an open option, on for mobile | [[22-implementation-decisions#Quick repair on mobile]] |

## Risks / Trade-offs

- [4.x changes the storage API] → the adapter is the only code that knows the engine ([[05-storage-trait]]).
- [The multi-process mode of 4.x tempts a design change] → out of scope; one owner process stays ([[adr:0002]]).
- [Binding crates pinned to older toolchains] → check them on 1.90 in the same change.
