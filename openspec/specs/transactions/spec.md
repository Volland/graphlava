# transactions Specification

## Purpose

How clients open a database, run transactions with a chosen durability level, and what survives a crash. Design: `tg.md/06-transactions-durability.md`, `tg.md/16-api.md`.

## Requirements

### Requirement: Single owner process
The system SHALL allow exactly one process at a time to open a database file; a second open from any process SHALL fail with a locked error.

Formal: none — OS locking lies outside the model; covered at level e2e

#### Scenario: Second process is refused
- **Level**: e2e
- **WHEN** process A has the file open and process B opens the same file
- **THEN** B receives a locked error and A is unaffected

### Requirement: Snapshot reads and one writer
The system SHALL run any number of read transactions on consistent snapshots concurrently with at most one write transaction.

Formal: none — covered at level e2e

#### Scenario: Reader does not see uncommitted writes
- **Level**: e2e
- **WHEN** a writer inserts a statement and has not committed
- **THEN** a concurrent reader does not see it

### Requirement: Per-transaction durability levels
The system SHALL accept a durability level `sync`, `ordered` or `relaxed` per write transaction and SHALL acknowledge a commit only when that level's guarantee holds.

Formal: `MetaCore.Durability.durable_survives`, `MetaCore.Durability.relaxed_covered`

#### Scenario: Sync commit survives a crash
- **Level**: crash
- **WHEN** a `sync` commit returns success and the process is killed
- **THEN** the data is present after reopening

#### Scenario: Ordered commit is acknowledged after its group is durable
- **Level**: crash
- **WHEN** several `ordered` commits are issued concurrently and each returns success, then the process is killed
- **THEN** every acknowledged commit is present after reopening

#### Scenario: Relaxed commit may be lost but never corrupts
- **Level**: crash
- **WHEN** a `relaxed` commit returns success and the process is killed before any later `sync` commit
- **THEN** after reopening the database is consistent and either contains the whole transaction or none of it

### Requirement: Batched commit
The system SHALL run the write requests of one group-commit batch in one engine write transaction committed once, SHALL make `sync` and `ordered` writes visible to readers only when they are durable, and SHALL isolate a failing request so that the rest of its batch commits.

Formal: `MetaCore.Durability.durable_survives` (batching itself is covered at levels e2e and crash)

#### Scenario: Ordered write is invisible until durable
- **Level**: e2e
- **WHEN** an `ordered` write runs in an open batch and a reader starts before the batch commits
- **THEN** the reader does not see the write, and a reader started after the acknowledgement does

#### Scenario: Failed request does not affect its batch
- **Level**: e2e
- **WHEN** three `ordered` requests share a batch and the second fails a compare-and-set
- **THEN** the first and third commit and are acknowledged, and the second reports a conflict and changes nothing

### Requirement: One-shot remote writes
The system SHALL accept writes from other processes only as complete write requests and SHALL limit how long an in-process interactive write transaction holds the writer.

Formal: none — covered at level e2e

#### Scenario: Slow in-process writer is cut off
- **Level**: e2e
- **WHEN** an in-process write transaction stays open beyond its time limit
- **THEN** its next call fails with a timeout, it is aborted, and queued requests proceed

### Requirement: Atomic transactions
The system SHALL apply all effects of a transaction, including cascades, index updates and ID allocation, atomically.

Formal: `MetaCore.Durability.recover`, `MetaCore.Durability.crash_outcome_exists`

#### Scenario: Crash during commit
- **Level**: crash
- **WHEN** the process is killed during a commit that inserts 1,000 statements
- **THEN** after reopening either all 1,000 or none are present, and no ID from the lost transaction appears anywhere

### Requirement: Portable file format
The system SHALL write one little-endian file format readable on Linux, macOS, Windows, iOS and Android, with a format version and feature flags, and SHALL refuse to open a file with an unknown feature flag.

Formal: none — byte layout; covered at level e2e and the golden-file round trip

#### Scenario: File moves between platforms
- **Level**: e2e
- **WHEN** a closed file written on macOS is opened on Android
- **THEN** all queries return the same results

#### Scenario: Unknown feature flag
- **Level**: e2e
- **WHEN** a file carries a feature flag the running build does not know
- **THEN** open fails with an incompatible-format error and the file is not modified
