# graphlava bindings

[graphlava](https://graphlava.org) is an embedded metagraph database for agent memory, written in Rust. These bindings bring the same engine, the same file and the same semantics to other languages.

| Language | Package | Built on | Source |
| --- | --- | --- | --- |
| Rust | `graphlava` (crate) | the engine itself | [crates/graphlava](../crates/graphlava/README.md) |
| C, C++, Zig, Go, Dart, … | `libgraphlava_ffi` + `graphlava.h` | the Rust API | [crates/graphlava-ffi](../crates/graphlava-ffi/README.md) |
| Python | `graphlava` on PyPI | PyO3 over the Rust API | [crates/graphlava-py](../crates/graphlava-py/README.md) |
| JavaScript / TypeScript (Node.js) | `@graphlava/node` on npm | napi-rs over the Rust API | [bindings/node](node/README.md) |
| Swift (iOS, macOS) | `GraphLava` Swift package | the C FFI | [bindings/swift](swift/README.md) |
| Kotlin (Android, JVM) | `org.graphlava:graphlava-android` | the C FFI via JNI | [bindings/kotlin](kotlin/README.md) |

Nothing is published to a registry yet; each README shows how to build from source.

## One behaviour everywhere

Every binding runs the same [conformance corpus](conformance/README.md): language-neutral JSON-lines cases for term and ID mapping, edge values (64-bit IDs, `-0`, `NaN`, 2^53, dates) and errors. They also replay the traces of the Lean model of the store, operation by operation, against a real file. The binding strategy is recorded in [ADR 0042](../docs/adr/0042-binding-strategy.md).

## Learn more

- [graphlava.org](https://graphlava.org) · [Mini book: Hypergraphs and Metagraphs in Statement Graphs](https://graphlava.org/book/) · [Articles](https://graphlava.org/articles/)
- [API design](../tg.md/16-api.md)
