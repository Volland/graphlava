/-
  # Durability and crash recovery (tg.md/06-transactions-durability.md, ADR 0012)

  A database is a sequence of committed transactions. A crash keeps some
  prefix of that sequence, never a part of a transaction, and never less than
  every transaction acknowledged at `sync` or `ordered`. redb commits are
  totally ordered, so a later durable commit makes every earlier commit durable.
-/
namespace MetaCore.Durability

inductive Level where
  | sync | ordered | relaxed
  deriving DecidableEq, Repr

/-- A level whose acknowledgment promises the transaction survives a crash. -/
def Level.durable : Level → Bool
  | .sync => true
  | .ordered => true
  | .relaxed => false

structure Txn (Op : Type) where
  level : Level
  ops : List Op

/-- Index just past the last acknowledged durable transaction. -/
def durableBound {Op : Type} : List (Txn Op) → Nat
  | [] => 0
  | t :: ts =>
    let rest := durableBound ts
    if rest > 0 then rest + 1 else if t.level.durable then 1 else 0

/-- What a crash may leave: the first `k` whole transactions, with
`durableBound log ≤ k ≤ log.length`. -/
structure CrashOutcome {Op : Type} (log : List (Txn Op)) (k : Nat) : Prop where
  lower : durableBound log ≤ k
  upper : k ≤ log.length

/-- The recovered state: apply whole transactions of the surviving prefix.
Atomicity (scenario "Crash during commit") holds by construction: a
transaction's ops are applied all together or not at all. -/
def recover {Op σ : Type} (apply : σ → Op → σ) (init : σ) (log : List (Txn Op)) (k : Nat) : σ :=
  ((log.take k).flatMap Txn.ops).foldl apply init

theorem durableBound_le {Op : Type} : ∀ log : List (Txn Op), durableBound log ≤ log.length
  | [] => by simp [durableBound]
  | t :: ts => by
    have ih := durableBound_le ts
    simp only [durableBound, List.length_cons]
    split
    · omega
    · split <;> omega

/-- Every durable transaction lies below the bound. -/
theorem durable_lt_bound {Op : Type} : ∀ (log : List (Txn Op)) (i : Nat) (hi : i < log.length),
    (log[i]'hi).level.durable = true → i < durableBound log
  | [], _, hi, _ => by simp at hi
  | t :: ts, 0, _, h => by
    simp only [durableBound]
    split
    · omega
    · simp at h; simp [h]
  | t :: ts, i + 1, hi, h => by
    have := durable_lt_bound ts i (by simpa using hi) (by simpa using h)
    simp only [durableBound]
    split <;> omega

/-- Scenarios "Sync commit survives a crash" and "Ordered commit is acknowledged
after its group is durable": every acknowledged durable transaction survives. -/
theorem durable_survives {Op : Type} {log : List (Txn Op)} {k i : Nat} (hc : CrashOutcome log k)
    (hi : i < log.length) (hd : (log[i]'hi).level.durable = true) : i < k :=
  Nat.lt_of_lt_of_le (durable_lt_bound log i hi hd) hc.lower

/-- A `relaxed` transaction followed by any later durable one also survives:
commits are ordered, so durability of a later commit covers earlier ones. -/
theorem relaxed_covered {Op : Type} {log : List (Txn Op)} {k i j : Nat} (hc : CrashOutcome log k)
    (hij : i < j) (hj : j < log.length) (hd : (log[j]'hj).level.durable = true) : i < k :=
  Nat.lt_trans hij (durable_survives hc hj hd)

/-- A crash outcome always exists: recovery is never stuck. -/
theorem crash_outcome_exists {Op : Type} (log : List (Txn Op)) :
    CrashOutcome log (durableBound log) := ⟨Nat.le_refl _, durableBound_le log⟩

/-- Crash state equals some state the database was in after a whole commit:
recovery to `k` is the same as replaying the first `k` commits one by one. -/
theorem recover_prefix {Op σ : Type} (apply : σ → Op → σ) (init : σ) (log : List (Txn Op)) (k : Nat) :
    recover apply init log k = recover apply init (log.take k) k := by
  simp [recover, List.take_take]

end MetaCore.Durability
