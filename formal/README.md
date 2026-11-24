# formal/ — Lean 4 model of meta-core

The executable specification of meta-core's observable state, with machine-checked
invariants. Strategy: `tg.md/19-verification.md`; theorem ↔ requirement ledger:
`tg.md/20-proofs.md`; decision: `docs/adr/0019-lean-model-and-refinement-testing.md`.

| Module | What it proves |
| --- | --- |
| `MetaCore/Id.lean` | kind tag round-trip and injectivity, chunk split, system graph ID, inline integer round-trip and order preservation, numeric subtype adjacency |
| `MetaCore/Store.lean` | `WF` (unique `(s,p,o)`, no duplicate IDs, IDs below the allocator) kept by insert / update / cascade delete; idempotent insert; update in place keeps ID and annotations; no dangling annotation after delete; allocator monotone |
| `MetaCore/Nesting.lean` | the cycle guard keeps nesting a DAG and is necessary; transitive scope composes |
| `MetaCore/Durability.lean` | a crash leaves a prefix of whole transactions containing every acknowledged durable commit |

```sh
lake build                    # check every proof (core Lean only, ~seconds)
./scripts/lean-audit.sh       # fail on sorry/admit/axiom or unexpected axioms
lake exe oracle 42 2000       # JSON-lines trace for the Rust refinement replay
```

Rules: no `sorry`, no `axiom`, no Mathlib. If the Rust code and this model
disagree, the model is the specification until a reviewed change updates it,
together with its theorems, the oracle and `tg.md/20-proofs.md`.
