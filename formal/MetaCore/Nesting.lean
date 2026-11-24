/-
  # Graph nesting and scope (tg.md/09-graphs.md, ADR 0007, ADR 0008)

  Nesting is a list of `(child, parent)` edges. A write that adds `child :in parent`
  is accepted only when it cannot close a cycle; the theorems below show that
  this guard is enough to keep nesting a DAG, and that transitive scope behaves
  as the spec says.
-/
namespace MetaCore.Nesting

abbrev Edges := List (Nat × Nat)

/-- `Reach E a b`: `a` is nested (transitively) under `b`. -/
inductive Reach (E : Edges) : Nat → Nat → Prop
  | edge {a b : Nat} : (a, b) ∈ E → Reach E a b
  | trans {a b c : Nat} : Reach E a b → Reach E b c → Reach E a c

def Acyclic (E : Edges) : Prop := ∀ g, ¬ Reach E g g

theorem Reach.mono {E F : Edges} (hsub : ∀ x ∈ E, x ∈ F) {a b : Nat} (h : Reach E a b) :
    Reach F a b := by
  induction h with
  | edge he => exact .edge (hsub _ he)
  | trans _ _ ih₁ ih₂ => exact .trans ih₁ ih₂

/-- Every path in `E + (a,b)` either avoids the new edge or goes through it. -/
theorem reach_cons {E : Edges} {a b x y : Nat} (h : Reach ((a, b) :: E) x y) :
    Reach E x y ∨ ((x = a ∨ Reach E x a) ∧ (b = y ∨ Reach E b y)) := by
  induction h with
  | @edge x y he =>
    simp only [List.mem_cons, Prod.mk.injEq] at he
    rcases he with ⟨rfl, rfl⟩ | he
    · exact Or.inr ⟨Or.inl rfl, Or.inl rfl⟩
    · exact Or.inl (.edge he)
  | @trans x y z _ _ ih₁ ih₂ =>
    rcases ih₁ with h₁ | ⟨hl₁, hr₁⟩ <;> rcases ih₂ with h₂ | ⟨hl₂, hr₂⟩
    · exact Or.inl (.trans h₁ h₂)
    · -- old path x→y, then y reaches a
      refine Or.inr ⟨?_, hr₂⟩
      rcases hl₂ with rfl | h
      · exact Or.inr h₁
      · exact Or.inr (.trans h₁ h)
    · refine Or.inr ⟨hl₁, ?_⟩
      rcases hr₁ with rfl | h
      · exact Or.inr h₂
      · exact Or.inr (.trans h h₂)
    · exact Or.inr ⟨hl₁, hr₂⟩

/-- Rule `dm-nesting-dag`: adding `child :in parent` when `parent` is not
already nested under `child` (and they differ) keeps nesting acyclic.
This is exactly the guard the write path runs (scenario "Cycle rejected"). -/
theorem addEdge_acyclic {E : Edges} {child parent : Nat}
    (hE : Acyclic E) (hne : child ≠ parent) (hguard : ¬ Reach E parent child) :
    Acyclic ((child, parent) :: E) := by
  intro g hg
  rcases reach_cons hg with h | ⟨hl, hr⟩
  · exact hE g h
  · rcases hl with rfl | hl <;> rcases hr with hr | hr
    · exact hne hr.symm
    · exact hguard hr
    · subst hr; exact hguard hl
    · exact hguard (.trans hr hl)

/-- If the guard is violated the edge really would close a cycle, so rejecting
it never refuses a legal write. -/
theorem guard_necessary {E : Edges} {child parent : Nat} (h : Reach E parent child) :
    ¬ Acyclic ((child, parent) :: E) := by
  intro hA
  exact hA child (.trans (.edge (List.mem_cons_self ..)) (h.mono (fun _ hx => List.mem_cons_of_mem _ hx)))

/-! ## Scope (rule `dm-scope-transitive`) -/

/-- A graph `h` is in the transitive scope of `g`. -/
def InScope (E : Edges) (g h : Nat) : Prop := h = g ∨ Reach E h g

/-- Direct scope is contained in transitive scope (scenario "Parent scope includes children"). -/
theorem direct_sub_transitive {E : Edges} {g h : Nat} (hd : (h, g) ∈ E) : InScope E g h :=
  Or.inr (.edge hd)

/-- Scope is transitive: everything in a child's scope is in the parent's scope. -/
theorem scope_trans {E : Edges} {g h k : Nat} (h₁ : InScope E g h) (h₂ : InScope E h k) :
    InScope E g k := by
  rcases h₁ with rfl | h₁ <;> rcases h₂ with rfl | h₂
  · exact Or.inl rfl
  · exact Or.inr h₂
  · exact Or.inr h₁
  · exact Or.inr (.trans h₂ h₁)

/- "Shared child counts once" is a property of the result *multiset* the
executor produces from `in_star`, not of `InScope`, which is already a set.
It is covered by an E2E scenario test, not by a theorem here. -/

end MetaCore.Nesting
