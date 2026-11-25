---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# iOS extension write journal

## Context and Problem Statement

iOS app extensions (share, widget, intents) are separate processes and must not open the database file: only the owner process does ([ADR-0002](0002-redb-single-owner-process.md)), and a suspended process holding a lock on an app-group file is killed with `0xdead10cc`. When the owner app is running, an extension can reach it over XPC; when it is not, the extension either queues the write or loses it. This is the open question "Extension write queue".

## Considered Options

<option id="per-extension-journal" outcome="chosen">

`GraphLava.ExtensionClient` sends a write request over XPC when it has an endpoint of the running app; otherwise it appends the request to its own journal `<appgroup>/graphlava/<extension-id>.journal`. The journal is a small file with its own magic (`GLVJ`) and version, a header with a base sequence number, and frames `len:u32le | seq:u64le | crc32c:u32le | request bytes`. On open the owner applies every complete frame as one write request and records `journal_seq:<extension-id> = seq` under a metadata key in the same transaction, so each frame applies exactly once even across a crash during apply. The first incomplete or corrupt frame ends the journal: it and the bytes after it are a torn tail, dropped with one warning. After a durable flush the owner truncates the journal to its header, raising the base sequence number so numbers are never reused. Appends and applies hold a short exclusive `flock` on the journal. The framing, checksum and apply logic live in the C FFI (`graphlava_journal_append`, `graphlava_journal_apply`), shared by every binding.

</option>

<option id="fail-the-write" outcome="rejected">

Fail the write when the app is not running: simple, but extensions lose data the user handed them.

</option>

<option id="open-from-extension" outcome="rejected">

Open the database from the extension: breaks the single-owner rule and risks `0xdead10cc` when the extension is suspended holding the lock.

</option>

<option id="shared-journal" outcome="rejected">

One journal shared by all extensions: two extensions can append at once, so it needs a cross-process protocol on every append; per-extension journals have one writer each.

</option>

## Decision Outcome

Chosen option: "per-extension-journal", because extension writes survive while the app is closed, the owner stays the only process that opens the database, and exactly-once application follows from committing the sequence number with the request.

### Consequences

<consequence kind="good">

Extension writes are never lost while the app is closed, and a crash while applying neither duplicates nor drops a complete frame.

</consequence>

<consequence kind="bad">

The journal format is a second small file format to version; it is outside the database file format, so it does not touch the v0 format freeze ([ADR-0018](0018-portable-file-format.md)).

</consequence>

<consequence kind="risk">

On iOS an extension rarely holds an XPC endpoint of its containing app, so the journal is the usual path; writes become visible only when the app next opens the database.

</consequence>

## More Information

Realised by `crates/graphlava-ffi/src/journal.rs`, `graphlava::Database::submit_once` and `bindings/swift/Sources/GraphLava/ExtensionClient.swift`; specified in [[16-api#Mobile bindings]] and requirement `[[openspec:mobile-bindings#iOS extension writes]]`. Link code with `// @tg: implements:: [[adr:0043]]`.
