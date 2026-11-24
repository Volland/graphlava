---
type: Constraint
source: "Apple, Microsoft, Android and Linux platform documentation; redb documentation"
specs: ["adr:0002", "adr:0012", "adr:0018"]
---
# Platform constraints

Rules the five target platforms impose on meta-core that nobody here chose: file locking, app suspension, page sizes, sync calls and file growth, and what each binds in the design.

- constrains:: [[06-transactions-durability#Durability levels]] {strength: must}
- constrains:: [[05-storage-trait#redb adapter]] {strength: must}

## Purpose

Linux, macOS, Windows, iOS and Android are first-class targets (handover D13). The owner-process model ([[adr:0002]]) and the portable format ([[adr:0018]]) depend on the rules below.

## All platforms

The on-disk format is identical everywhere: little-endian records, big-endian keys, no OS-specific fields ([[adr:0018]]).

- constrains:: [[02-id-encoding#Bit layout]] {strength: must}
- constrains:: [[04-physical-layout#Key byte order]] {strength: must}

A database file may be copied between platforms only while it is closed, or through a snapshot export. redb's file format is cross-platform; this is verified by opening the same file on all five platforms in CI.

## iOS

Apple terminates a suspended app that holds a file lock on a file in a shared app-group container (crash code `0xdead10cc`). Apps have limited virtual address space and run extensions as separate processes.

- constrains:: [[06-transactions-durability#Group commit]] {strength: must}
- constrains:: [[16-api#Mobile bindings]] {strength: must}

The owner process must, on the "will resign active" or background notification, close the open batch with a flush, finish or abort the running write transaction, and close the database (releasing redb's file lock) when the file is in an app-group container. On return to foreground it reopens the file and re-pins readers.

App extensions (share, widget, intents) are separate processes. They must not open the file; they reach the owner app through XPC or a shared request queue that the app drains, per [[adr:0002]].

Durable sync on Apple platforms needs `F_FULLFSYNC`; plain `fsync` does not flush the drive cache. Whether redb issues `F_FULLFSYNC` for `Durability::Immediate` is verified in the engine spike.

<exception id="ios-private-container" approved_by="Volodia">

A database file in the app's private container (not an app-group container) is not subject to `0xdead10cc`; the owner may keep it open in the background, but must still flush the open batch.

</exception>

Background execution time on iOS is short (a few seconds after backgrounding, per Apple documentation) so long writes such as large graph drops or sweeps must be resumable.

## Android

Android devices ship with 4 KB or 16 KB memory pages, and from 2025 Google Play requires native libraries to support 16 KB pages. Apps run in one process by default and may be killed in the background without warning.

- constrains:: [[16-api#Mobile bindings]] {strength: must}

Native libraries are built with the NDK and linked with 16 KB-aligned ELF segments. redb is pure Rust, so no C toolchain is needed beyond the NDK linker.

A kill without warning is a crash for meta-core; durability follows [[06-transactions-durability#Crash recovery]]. Apps that need writes to survive a background kill use `sync` or `ordered`, not `relaxed`.

Whether redb's page size can be set to 16 KB, and whether it matters for performance on 16 KB devices, is to be measured.

## Windows

Windows locks are mandatory: a byte-range lock blocks other readers and writers of that range. Memory-mapped files cannot grow while mapped. Durable writes need `FlushFileBuffers`.

- constrains:: [[05-storage-trait#redb adapter]] {strength: must}

The current redb versions use file reads and writes with their own page cache rather than a memory map; this is to be verified in the engine spike. If true, mapped-file growth does not apply. Any own pager written later behind the `Storage` trait must handle growth and must place lock bytes outside the data region.

File paths must support long paths (`\\?\` prefix) and non-ASCII names.

## macOS

macOS shares Apple's sync rules: `fsync` does not flush the drive cache, `F_FULLFSYNC` does, and `F_BARRIERFSYNC` orders writes without a full flush.

- constrains:: [[06-transactions-durability#Durability levels]] {strength: must}

`sync` and the flush that closes an `ordered` batch must reach stable storage, which on macOS means `F_FULLFSYNC` or equivalent. `F_BARRIERFSYNC` is not used in v0 ([[adr:0012]]).

## Linux

Linux has advisory locks, `fsync` and `fdatasync` that flush through to the device on correctly configured file systems, and no restriction on file growth.

Linux is the reference platform for crash and power-loss testing, because fault-injecting block devices are available there.

## Open items

Questions these constraints leave open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Page size on 16 KB devices]]
- [[open-questions#Large drops on mobile]]
