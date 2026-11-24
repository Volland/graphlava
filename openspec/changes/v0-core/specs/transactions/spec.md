# Spec Delta

## Purpose

How clients open a database, run transactions with a chosen durability level, and what survives a crash. Design: `tg.md/06-transactions-durability.md`, `tg.md/16-api.md`.

## ADDED Requirements

### Requirement: Single owner process
The system SHALL allow exactly one process at a time to open a database file; a second open from any process SHALL fail with a locked error.

#### Scenario: Second process is refused
- **WHEN** process A has the file open and process B opens the same file
- **THEN** B receives a locked error and A is unaffected

### Requirement: Snapshot reads and one writer
The system SHALL run any number of read transactions on consistent snapshots concurrently with at most one write transaction.

#### Scenario: Reader does not see uncommitted writes
- **WHEN** a writer inserts a statement and has not committed
- **THEN** a concurrent reader does not see it

### Requirement: Per-transaction durability levels
The system SHALL accept a durability level `sync`, `ordered` or `relaxed` per write transaction and SHALL acknowledge a commit only when that level's guarantee holds.

#### Scenario: Sync commit survives a crash
- **WHEN** a `sync` commit returns success and the process is killed
- **THEN** the data is present after reopening

#### Scenario: Ordered commit is acknowledged after its group is durable
- **WHEN** several `ordered` commits are issued concurrently and each returns success, then the process is killed
- **THEN** every acknowledged commit is present after reopening

#### Scenario: Relaxed commit may be lost but never corrupts
- **WHEN** a `relaxed` commit returns success and the process is killed before any later `sync` commit
- **THEN** after reopening the database is consistent and either contains the whole transaction or none of it

### Requirement: Atomic transactions
The system SHALL apply all effects of a transaction, including cascades, index updates and ID allocation, atomically.

#### Scenario: Crash during commit
- **WHEN** the process is killed during a commit that inserts 1,000 statements
- **THEN** after reopening either all 1,000 or none are present, and no ID from the lost transaction appears anywhere

### Requirement: Portable file format
The system SHALL write one little-endian file format readable on Linux, macOS, Windows, iOS and Android, with a format version and feature flags, and SHALL refuse to open a file with an unknown feature flag.

#### Scenario: File moves between platforms
- **WHEN** a closed file written on macOS is opened on Android
- **THEN** all queries return the same results

#### Scenario: Unknown feature flag
- **WHEN** a file carries a feature flag the running build does not know
- **THEN** open fails with an incompatible-format error and the file is not modified
