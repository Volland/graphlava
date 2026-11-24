# Proposal

## Why

On-device agents need memory that survives app suspension and background kills. [[16-api#Mobile bindings]] plans Swift and Kotlin bindings for v1 ([[adr:0013]]), but the C FFI they wrap exports only a version string today. iOS kills a suspended app that holds a lock on a file in an app-group container (`0xdead10cc`). Android kills apps without warning and requires 16 KB-page native libraries ([[07-platforms#iOS]], [[07-platforms#Android]]).

## What Changes

- **C FFI:** completes the five function groups of [[16-api#C FFI]], including `graphlava_suspend` and `graphlava_resume`. Adds a cbindgen header versioned by `GRAPHLAVA_ABI_VERSION`, 1024-row ID batches, and no panics across the boundary.
- **Swift:** the `GraphLava` package over an XCFramework. It provides a `Database` actor, closure-scoped transactions, and `AsyncSequence` rows. It also calls suspend and resume automatically on app lifecycle events.
- **Kotlin:** the `org.graphlava:graphlava-android` AAR, linked for 16 KB pages. It provides `suspend` functions on dedicated dispatchers, `Flow<RowBatch>` results, and a flush on `ProcessLifecycleOwner` `ON_STOP`.
- **iOS app extensions:** `ExtensionClient` writes through XPC, or through a per-extension journal that the app applies exactly once. This resolves [[open-questions#Extension write queue]] with its recommended option, recorded in ADR 0043.
- **Mobile benchmark:** fails a run if suspend p99 is 500 ms or more at 10M statements.

Out of scope:
- Opening the file from an extension process ([[adr:0002]]).
- Resumable large drops ([[open-questions#Large drops on mobile]], tracked separately).
- watchOS, tvOS and Android 32-bit ABIs.

## Capabilities

### New Capabilities

- `mobile-bindings`: the complete C FFI, the Swift and Kotlin APIs, iOS suspend and resume, Android kill safety, iOS extension writes, native packaging, and mobile latency.

### Modified Capabilities

None.

## Impact

- **Code:** `crates/graphlava-ffi` (the full C API, `graphlava.h`), `bindings/swift/`, `bindings/kotlin/`, and CI jobs for the XCFramework, the AAR, the simulator and emulator tests, and the nightly device runner.
- **Dependencies:** `cbindgen`, and `cargo-fuzz` (dev only). SwiftPM, Gradle and the Android NDK are used in CI only.
- **Depends on:** `v1-python-binding` group 1 (ADR 0042, the conformance corpus).
- **lat.md sections:**
  - [[16-api#C FFI]] and [[16-api#Mobile bindings]] (extension journal).
  - [[07-platforms#iOS]] and [[07-platforms#Android]].
  - [[17-benchmarks#Mobile]].
  - [[21-e2e-tests#C FFI lifecycle]] plus new suspend and journal scenarios.
  - [[open-questions#Extension write queue]] and [[resolved-questions]].
- **Binding constraints:**
  - The [[16-api]] rules `api-ffi-abi`, `api-ffi-rows`, `api-ios-suspend`, `api-ios-extension-ipc`, `api-kotlin-threads` and `api-single-owner`.
  - The `ios-private-container` exception.
  - [[07-platforms#iOS]] and [[07-platforms#Android]] each `constrains` [[16-api#Mobile bindings]].
- **Requirements added:** all are `[[openspec:mobile-bindings#…]]`:
  - Complete C FFI
  - FFI misuse safety
  - Swift API
  - Kotlin API
  - iOS suspend and resume
  - Android background and kill safety
  - Mobile native packaging
  - iOS extension writes
  - Mobile latency
- **File format:** the database file is unchanged. The extension journal is a separate file with its own magic and version. The owner records the last applied journal sequence under a metadata key, which is a new key in an existing table and needs no format bump.
- **Performance:**
  - [[17-benchmarks#Mobile]] gates suspend p99 below 500 ms.
  - `Binding overhead` covers the C FFI on the host.
  - [[17-benchmarks#Durability commit latency]] could regress if suspend adds work, so it also runs through the FFI.
- **Verification:** `Formal: none` for every requirement, because suspend relies on `MetaCore.Durability` and adds no invariant. Coverage by level:
  - L1: Swift oracle replay for seeds 1..8.
  - L2: FFI fuzzing and torn-journal fuzzing.
  - L3: simulator and emulator e2e tests.
  - L4: background kill, and a kill while the journal is being applied.
  - L6: Mobile.
