# formal/ — Lean 4 model of graphlava

The executable specification of graphlava's observable state, with machine-checked
invariants. Strategy: `tg.md/19-verification.md`; theorem ↔ requirement ledger:
`tg.md/20-proofs.md`; decision: `docs/adr/0019-lean-model-and-refinement-testing.md`.

| Module | What it proves |
| --- | --- |
| `MetaCore/Id.lean` | kind tag round-trip and injectivity, chunk split, system graph ID, inline integer round-trip and order preservation, numeric subtype adjacency |
| `MetaCore/Store.lean` | `WF` (unique `(s,p,o)`, no duplicate IDs, IDs below the allocator) kept by insert / update / cascade delete; idempotent insert; update in place keeps ID and annotations; no dangling annotation after delete; allocator monotone |
| `MetaCore/Nesting.lean` | the cycle guard keeps nesting a DAG and is necessary; transitive scope composes |
| `MetaCore/Hyperedge.lean` | hyperedges on the shared allocator: shape validation (declared, at most once, required present) and arity; cascade delete with the role guard of `he-delete-filler` keeps `HWF` and leaves no dangling subject, object or role filler; `DETACH` never blocks |
| `MetaCore/Graphs.lean` | the oracle step function `apply`: every live statement keeps a non-empty membership set, nesting stays acyclic, nest is rejected exactly on a cycle, scope equals `InScope` and lists each ID once; the full invariant holds after every operation |
| `MetaCore/Datalog.lean` | rule safety check equals its definition; the stratifier accepts exactly the programs with no cycle through negation (= those with a valid stratification) and its stratum assignment is valid; per stratum, naive evaluation reaches the least fixpoint within `|HB| + 1` rounds, semi-naive evaluation computes the same set, for safe rules grounding over the active domain loses nothing; a stratified program has one stable model, and bottom-up evaluation along any valid stratification computes it |
| `MetaCore/Durability.lean` | a crash leaves a prefix of whole transactions containing every acknowledged durable commit |

```sh
lake build                    # check every proof (core Lean only, ~seconds)
./scripts/lean-audit.sh       # fail on sorry/admit/axiom or unexpected axioms
lake exe oracle 42 2000       # JSON-lines trace for the Rust refinement replay
```

Rules: no `sorry`, no `axiom`, no Mathlib. If the Rust code and this model
disagree, the model is the specification until a reviewed change updates it,
together with its theorems, the oracle and `tg.md/20-proofs.md`.
