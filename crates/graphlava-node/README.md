# graphlava-node

The native half of [`@graphlava/node`](https://github.com/Volland/graphlava/tree/main/bindings/node), the Node.js binding of [graphlava](https://graphlava.org): a [napi-rs](https://napi.rs) crate (N-API 9) over the Rust API of the [`graphlava`](https://github.com/Volland/graphlava/tree/main/crates/graphlava) crate.

You do not use this crate directly. Install the npm package instead:

```sh
npm install @graphlava/node
```

The JavaScript layer, the TypeScript types, the tests and the README with examples live in [`bindings/node/`](https://github.com/Volland/graphlava/tree/main/bindings/node). Prebuilt binaries are published as `@graphlava/node-<platform>-<arch>` packages, one per target.

## Building from source

```sh
cd bindings/node
npm install
npm run build      # builds this crate with @napi-rs/cli into graphlava.<target>.node
npm test           # node:test suites, the conformance corpus and the oracle replay
```

Rust 1.88 or newer (napi 3 needs it; the core crate needs 1.87).

## Learn more

- [graphlava.org](https://graphlava.org) · [Mini book](https://graphlava.org/book/) · [Articles](https://graphlava.org/articles/)
- [Binding strategy (ADR 0042)](https://github.com/Volland/graphlava/blob/main/docs/adr/0042-binding-strategy.md)

## License

MIT OR Apache-2.0.
