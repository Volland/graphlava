---
status: proposed
date: 2026-11-24
decision-makers: [Volodia]
---
# Lean 4 model with machine-checked invariants, tied to Rust by refinement tests

## Context and Problem Statement

meta-core promises universal properties that tests can only sample: IDs are never reused, statements stay unique, nesting stays acyclic, a crash leaves a durable prefix. The file format freezes at the end of v0 ([ADR-0018](0018-portable-file-format.md)), so a mistake in these rules becomes permanent. We need an argument that covers every state, and a way to connect it to the Rust code.

## Considered Options

<option id="lean-model-refinement" outcome="chosen">

A Lean 4 model of the observable state (`formal/`, core Lean only) with theorems for each invariant, plus an executable oracle whose traces the Rust store must replay exactly. Proofs cover all inputs of the model; replay ties the model to the code; tests cover what the model abstracts.

</option>

<option id="tla-plus" outcome="deferred">

TLA+ with TLC model checking. Strong for concurrency protocols such as group commit and the sweep race, weaker for data invariants over unbounded state. Deferred: may be added for [[06-transactions-durability#Group commit]] if the crash sweep finds protocol bugs.

</option>

<option id="verify-rust-directly" outcome="deferred">

Prove the Rust code directly with Verus or Kani. Kani is a good fit for bounded checks of the ID codecs and may be added there; full verification of a storage engine on redb is out of reach for v0.

</option>

<option id="tests-only" outcome="rejected">

Property, E2E and crash tests only. Cheaper, but an untested corner of an invariant ships silently and then freezes into the format.

</option>

## Decision Outcome

Chosen option: "lean-model-refinement", because it gives all-input guarantees for the rules that freeze into the format, while the refinement replay keeps the proofs honest about the real code.

### Consequences

<consequence kind="good">

Each invariant requirement has a theorem or an explicit open obligation ([[20-proofs]]); the model doubles as an executable reference for differential testing.

</consequence>

<consequence kind="bad">

Contributors need basic Lean; every model change updates theorems, the oracle and the ledger together. The proofs cover the model, so the abstraction function and replay must be maintained with care.

</consequence>

<consequence kind="risk">

Model drift: a Rust change that the replay generator never exercises can diverge silently. Mitigation: the generator covers every operation, and failing seeds are kept as regressions.

</consequence>

## More Information

Strategy and assurance levels: `tg.md/19-verification.md`. Proof ledger: `tg.md/20-proofs.md`. Audit script: `formal/scripts/lean-audit.sh`.
