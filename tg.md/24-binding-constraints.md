---
type: Constraint
source: "Python Packaging Authority and CPython PEPs (384, 600, 703, 561); host runtime documentation"
specs: ["adr:0042"]
---
# Binding constraints

External rules the host-language bindings must respect and nobody here chose: packaging policies and interpreter ABIs. Each section names what it binds.

## CPython stable ABI

CPython's limited API and stable ABI (PEP 384, abi3) let one wheel per target serve every CPython from 3.10 on, but restrict the C API a binding may use.

- constrains:: [[openspec:python-binding#Python packaging]] {strength: must}
- constrains:: [[23-binding-decisions#Python binds the Rust API with PyO3]] {strength: must}

## Free-threaded CPython

PEP 703 free-threading (CPython 3.14t) runs Python threads without a GIL unless an extension module asks for it. The stable ABI does not cover it, so 3.14t needs its own wheel, and the handle must be `Send + Sync`.

- constrains:: [[openspec:python-binding#GIL released during core work]] {strength: must}
- constrains:: [[openspec:python-binding#Python packaging]] {strength: must}

## Linux wheel policy

Linux wheels follow manylinux_2_28 (PEP 600, glibc 2.28) and musllinux_1_2 (PEP 656), checked by `auditwheel`.

- constrains:: [[openspec:python-binding#Python packaging]] {strength: must}

## Typed packages

PEP 561: a package ships `py.typed` and its `.pyi` stubs inside the wheel for type checkers to use them.

- constrains:: [[openspec:python-binding#Python packaging]] {strength: must}

## Node.js event loop

Node.js runs all JavaScript of a thread on one libuv event loop; a blocking call on it stops every timer, socket and request of that thread, and libuv's shared pool has 4 threads that `fs` and `dns` also use.

- constrains:: [[openspec:node-binding#Event loop is never blocked]] {strength: must}
- constrains:: [[23-binding-decisions#Node runs database work on its own threads]] {strength: must}

## JavaScript numbers

A JavaScript `number` is an IEEE 754 double: integers are exact only inside ±(2^53−1), while graphlava IDs use all 64 bits. `bigint` holds any integer.

- constrains:: [[openspec:node-binding#Node ID mapping]] {strength: must}
- constrains:: [[openspec:node-binding#Node literal mapping]] {strength: must}
- constrains:: [[23-binding-decisions#Node IDs are bigint and integers are numbers only when safe]] {strength: must}

## Node worker threads

`worker_threads` run in the same process and load the same native addon, so per-process OS file locks do not separate them; structured clone copies plain objects between them.

- constrains:: [[openspec:node-binding#Single owner across workers and processes]] {strength: must}
- constrains:: [[23-binding-decisions#Node keeps one handle per file per process]] {strength: must}

## N-API ABI stability

N-API 9 is ABI-stable across Node.js releases, so one prebuilt binary per target serves Node 20, 22 and 24.

- constrains:: [[openspec:node-binding#Node packaging]] {strength: must}

## Node.js release schedule

Node.js 20, 22 and 24 are the supported release lines; the CI matrix tests each on every prebuilt target.

- constrains:: [[openspec:node-binding#Node packaging]] {strength: must}

## npm per-target packages

npm installs only the `optionalDependencies` whose `os`, `cpu` and `libc` fields match the host, so each prebuilt binary ships as its own package `@graphlava/node-<platform>-<arch>[-<abi>]`.

Scoped packages are private by default: every package.json sets `"publishConfig": { "access": "public" }`.

- constrains:: [[openspec:node-binding#Node packaging]] {strength: must}

## Explicit resource management

TC39 explicit resource management (`Symbol.asyncDispose`, `await using`) disposes an object on scope exit, also when the scope throws; Node 24 parses `await using`, and Node 20 and 22 define the symbol.

- constrains:: [[openspec:node-binding#Interactive write transactions in Node]] {strength: must}
- constrains:: [[23-binding-decisions#Node write limit runs across awaits]] {strength: should}

## Google Play 16 KB pages

From 2025 Google Play requires native libraries of new apps to load on devices with 16 KB memory pages: every `LOAD` segment aligned to 0x4000 (`-z max-page-size=16384`).

- constrains:: [[openspec:mobile-bindings#Mobile native packaging]] {strength: must}

## Apple privacy manifest

Apple requires a privacy manifest (`PrivacyInfo.xcprivacy`) in SDKs that call required-reason APIs; the engine reads file metadata (file-timestamp category).

- constrains:: [[openspec:mobile-bindings#Mobile native packaging]] {strength: must}

## App extension limits

iOS app extensions run as separate processes with tight memory and runtime limits and are suspended without notice; an extension that holds a lock on an app-group file when suspended is killed (`0xdead10cc`).

- constrains:: [[openspec:mobile-bindings#iOS extension writes]] {strength: must}
- constrains:: [[23-binding-decisions#iOS extensions write through XPC or a journal]] {strength: must}

## Android main thread

Android reports an ANR when the main thread is blocked for about 5 seconds, and StrictMode flags disk reads on it.

- constrains:: [[openspec:mobile-bindings#Kotlin API]] {strength: must}
- constrains:: [[23-binding-decisions#Mobile threading is an actor in Swift and dispatchers in Kotlin]] {strength: must}
