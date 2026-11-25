# Tasks

Builds on the archived `2026-11-24-v0-query` change (task 4.1) and depends on `v1-python-binding` group 1 (ADR 0042, the conformance corpus).

## 1. Specs and decisions

- [x] 1.1 Write ADR 0043 "iOS extension write journal" (D4), and move [[open-questions#Extension write queue]] to [[resolved-questions]].
- [x] 1.2 Record D2–D5 as `type: Decision` sections and the platform constraints as `type: Constraint` sections, with `motivated_by::` and `constrains:: [[openspec:mobile-bindings#<Requirement>]] {strength: must}` edges.
- [x] 1.3 Extend [[16-api#Mobile bindings]] with the journal, and [[16-api#C FFI]] with `GRAPHLAVA_ABI_VERSION`, `GRAPHLAVA_ERR_ABI` and `GRAPHLAVA_ERR_CLOSED`. Add the two `<assumption>` entries to [[20-proofs]].
- [x] 1.4 Add the iOS suspend, Android kill and extension journal scenarios to [[21-e2e-tests]], and extend [[17-benchmarks#Mobile]] with the suspend gate.

## 2. C FFI

- [x] 2.1 Write the failing C lifecycle test under ASan/LSan, the use-after-close test, and the `cargo fuzz` call-sequence target (red). (The core suspend tests were written red first; the C programs `tests/c/lifecycle.c` and `tests/c/misuse.c` and the fuzz targets `ffi_calls` and `journal_frames` were written alongside the FFI.)
- [x] 2.2 Implement the lifecycle, transactions, core, queries and admin groups with `catch_unwind` on every entry point. (Handles are registry tokens. Query parameters are not offered: the core has none; [[16-api#C FFI]] says so.)
- [x] 2.3 Implement the `graphlava_rows_next_batch` 1024-row batches and `graphlava_term_decode`.
- [x] 2.4 Generate `graphlava.h` with cbindgen, add `GRAPHLAVA_ABI_VERSION` checking, and fail CI on header drift. Annotate `[[openspec:mobile-bindings#Complete C FFI]]` and `[[openspec:mobile-bindings#FFI misuse safety]]`.
- [x] 2.5 Implement `graphlava_suspend` and `graphlava_resume` in the core (abort the interactive write, flush the batch, end the snapshots, release the lock for an app-group file) with unit tests for every step order.

## 3. Swift

- [ ] 3.1 Write the failing XCTest suite (simulator and macOS host): one test per Swift and iOS scenario, the oracle replay for seeds 1..8, and a host app with a share-extension target (red). — Not done: the XCTest suite (one test per Swift and iOS scenario, oracle replay seeds 1..8) passes on the macOS host and compiles for the iOS simulator and device, but no simulator runtime is installed here, so it ran on no simulator; the host app with a share-extension target is not written. CI runs the simulator job (`.github/workflows/mobile.yml`).
- [x] 3.2 Build the XCFramework (5 slices) with the module map and the privacy manifest, and the `GraphLava` Swift package.
- [x] 3.3 Implement the `Database` actor, closure transactions, `Term`, `GraphLavaError` and `AsyncSequence` rows. Annotate `[[openspec:mobile-bindings#Swift API]]`.
- [x] 3.4 Implement the lifecycle observer with `beginBackgroundTask`, `graphlava_suspend`/`graphlava_resume` and the private-container check. Annotate `[[openspec:mobile-bindings#iOS suspend and resume]]`.
- [x] 3.5 Implement the journal writer in `ExtensionClient`, with framing, CRC32C and `seq`, plus the torn-frame fuzz test. (Framing, CRC-32C and `seq` live in the C FFI, `crates/graphlava-ffi/src/journal.rs`, shared by every binding; `ExtensionClient` calls `graphlava_journal_append`.)
- [x] 3.6 Implement journal apply on open with `journal_seq`, torn-tail drop and truncation, and the extension-process guard on `Database.open`.
- [x] 3.7 Implement the XPC path in `ExtensionClient`. Annotate `[[openspec:mobile-bindings#iOS extension writes]]`. (Verified on the macOS host with an anonymous `NSXPCListener`; on iOS an extension rarely holds the app's endpoint, so the journal is the usual path.)
- [ ] 3.8 Run the background-during-group-commit crash test and the kill-during-journal-apply test on the nightly device runner, with the `0xdead10cc` crash-log check. — Not done: needs the reference iPhone and the nightly device runner (job `devices-nightly` is configured but its harness is not written).

## 4. Kotlin

- [ ] 4.1 Write the failing tests: JVM host oracle replay, instrumented tests on an arm64-v8a emulator and a 16 KB-page Android 15 image, and the kill-after-background test (red). — Partly done: JVM host oracle replay and kill-after-background (SIGKILL of a child JVM) pass; the instrumented tests (`graphlava-android/src/androidTest`) are written but not run, because the local API 34 arm64 image is incomplete and no 16 KB Android 15 image is installed. CI runs them on emulators.
- [x] 4.2 Write the JNI shim and the Gradle build of the AAR (`arm64-v8a`, `x86_64`, `-z max-page-size=16384`), with the `llvm-readelf` alignment gate. Annotate `[[openspec:mobile-bindings#Mobile native packaging]]`.
- [x] 4.3 Implement the `suspend` API, dispatchers, the main-thread guard, `Flow<RowBatch>`, `ULong` IDs and the sealed exceptions. Annotate `[[openspec:mobile-bindings#Kotlin API]]`.
- [x] 4.4 Implement the `ProcessLifecycleOwner` `ON_STOP` flush. Annotate `[[openspec:mobile-bindings#Android background and kill safety]]`.

## 5. Benchmarks

- [ ] 5.1 Add the FFI host run to `Binding overhead` and record its baseline. — Not done: `Binding overhead` belongs to `v1-python-binding`; no FFI host run or baseline was added.
- [ ] 5.2 Run [[17-benchmarks#Mobile]] on the reference iPhone and Android device at 1M and 10M statements, store the baselines, and gate suspend p99 below 500 ms. Annotate `[[openspec:mobile-bindings#Mobile latency]]`. — Not done: needs the reference devices and the nightly device runner.

## 6. Annotations and checks

- [x] 6.1 Annotate every C, Swift and Kotlin test with `@tg: verifies:: [[openspec:mobile-bindings#<Requirement>#<Scenario>]] {level: …}`.
- [x] 6.2 Add `mobile-bindings` to the `openspec:` frontmatter of [[16-api]].
- [ ] 6.3 Notify the `site/` session about the Swift package and the AAR. — Left for the parent session.
- [ ] 6.4 Run `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `swift test`, `./gradlew check`, `lake build`, `formal/scripts/lean-audit.sh`, `tg check`, `tg ontology check` and `tg trace mobile-bindings --gaps`, and close every gap they report. — Run: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`, `swift test`, `./gradlew check`, `tg check` (no new finding classes), `tg ontology check`, `openspec validate --strict`. `formal/` is untouched, so no Lean run. Open gaps in `tg trace mobile-bindings --gaps`: the device scenarios of 3.8 and 5.2.
