/-
  # Datalog: safety, stratification, naive and semi-naive evaluation
  (tg.md/13-datalog.md, rules `dl-safety`, `dl-stratified`, `dl-semi-naive`)

  Programs over ground facts: predicates and constants are `Nat`, atoms carry
  variables or constants, rules have positive and negated body atoms.

  * Safety (`dl-safety`): `Safe` and the executable `safeB`, proved equal.
  * Stratification (`dl-stratified`): the predicate dependency graph has an
    edge `head → body` per body atom, positive or negative. `stratB` rejects a
    program exactly when some negative edge lies on a cycle (PO-STRAT), and for
    an accepted program `stratum` is a valid stratification: positive edges go to
    a stratum `≤`, negative edges to a stratum `<`. `stratB` is sound and
    complete with respect to "a valid stratification exists".
  * Evaluation of one stratum (`dl-semi-naive`): the rules are grounded over a
    finite active domain, with negated atoms decided against the complete lower
    strata `low`. On the ground program, naive iteration reaches the least
    fixpoint (it equals the inductive `Derivable` set) within `|HB| + 1`
    rounds, where `HB` is the finite list of ground heads, and semi-naive
    iteration (each round fires only ground rules with a body atom in the
    previous delta) computes the same set (PO-SEMINAIVE).
  * Whole programs (PO-STRAT): a stratified program has at most one stable
    model (`Stable`: the least model of its Gelfond–Lifschitz reduct), and
    bottom-up evaluation along any valid stratification yields it, so the result
    does not depend on the stratification chosen.

  Not modelled: aggregation (for stratification it is one more negative-like
  edge; adding it to `negEdges` leaves every theorem below unchanged), built-ins,
  magic sets, the join order and sorted-relation layout of the Rust engine.
  The identification of the stable model with Przymusinski's perfect model is
  the classical result for stratified programs and is not formalised here.
-/
import MetaCore.Graphs

namespace MetaCore.Datalog

open MetaCore.Nesting (Edges Reach)
open MetaCore.Graphs (reachB reachB_iff)

/-! ## Syntax -/

inductive Term where
  | var (v : Nat)
  | const (c : Nat)
  deriving DecidableEq, Repr

structure Atom where
  pred : Nat
  args : List Term
  deriving DecidableEq, Repr

structure Rule where
  head : Atom
  pos : List Atom
  neg : List Atom := []
  deriving Repr

abbrev Program := List Rule

/-- A ground fact: predicate and constant arguments. -/
abbrev Fact := Nat × List Nat
abbrev DB := List Fact

def Term.vars : Term → List Nat
  | .var v => [v]
  | .const _ => []

def Atom.vars (a : Atom) : List Nat := a.args.flatMap Term.vars

def Rule.vars (r : Rule) : List Nat :=
  r.head.vars ++ r.pos.flatMap Atom.vars ++ r.neg.flatMap Atom.vars

/-! ## Safety (rule `dl-safety`) -/

/-- Every head variable and every variable of a negated atom occurs in a
positive body atom. -/
def Safe (r : Rule) : Prop :=
  ∀ v, (v ∈ r.head.vars ∨ ∃ a ∈ r.neg, v ∈ a.vars) → ∃ a ∈ r.pos, v ∈ a.vars

def safeB (r : Rule) : Bool :=
  (r.head.vars ++ r.neg.flatMap Atom.vars).all (fun v => r.pos.any (fun a => a.vars.contains v))

theorem safeB_iff (r : Rule) : safeB r = true ↔ Safe r := by
  simp only [safeB, Safe, List.all_eq_true, List.any_eq_true, List.contains_iff_mem,
    List.mem_append, List.mem_flatMap]

def programSafeB (P : Program) : Bool := P.all safeB

theorem programSafeB_iff (P : Program) : programSafeB P = true ↔ ∀ r ∈ P, Safe r := by
  simp only [programSafeB, List.all_eq_true, safeB_iff]

/-! ## Dependency graph and stratification (rule `dl-stratified`) -/

/-- Positive dependency edges `(head predicate, body predicate)`. -/
def posEdges (P : Program) : Edges := P.flatMap (fun r => r.pos.map (fun a => (r.head.pred, a.pred)))

/-- Negative dependency edges. -/
def negEdges (P : Program) : Edges := P.flatMap (fun r => r.neg.map (fun a => (r.head.pred, a.pred)))

def deps (P : Program) : Edges := posEdges P ++ negEdges P

/-- `s` is a valid stratification: a rule never depends positively on a higher
stratum, nor negatively on its own or a higher stratum. -/
def IsStrat (P : Program) (s : Nat → Nat) : Prop :=
  (∀ e ∈ posEdges P, s e.2 ≤ s e.1) ∧ (∀ e ∈ negEdges P, s e.2 < s e.1)

/-- Some cycle of the dependency graph passes through a negative edge. -/
def NegCycle (P : Program) : Prop :=
  ∃ e ∈ negEdges P, e.2 = e.1 ∨ Reach (deps P) e.2 e.1

/-- The stratifier's check: no negative edge closes a cycle. -/
def stratB (P : Program) : Bool :=
  (negEdges P).all (fun e => !(e.2 == e.1 || reachB (deps P) e.2 e.1))

theorem stratB_iff_not_negCycle (P : Program) : stratB P = true ↔ ¬ NegCycle P := by
  simp only [stratB, NegCycle, List.all_eq_true, Bool.not_eq_true', Bool.or_eq_false_iff,
    beq_eq_false_iff_ne, not_exists, not_and, not_or]
  constructor
  · intro h e he
    have := h e he
    exact ⟨this.1, fun hr => by rw [reachB_iff.mpr hr] at this; exact absurd this.2 (by simp)⟩
  · intro h e he
    refine ⟨(h e he).1, ?_⟩
    cases hb : reachB (deps P) e.2 e.1
    · rfl
    · exact absurd (reachB_iff.mp hb) (h e he).2

/-- A cycle through negation is rejected. -/
theorem negCycle_rejected {P : Program} (h : NegCycle P) : stratB P = false := by
  cases hs : stratB P
  · rfl
  · exact absurd h ((stratB_iff_not_negCycle P).mp hs)

/-- `p` reaches `q` by a path that uses at least one negative edge. -/
def NegReach (P : Program) (p q : Nat) : Prop :=
  ∃ e ∈ negEdges P, (p = e.1 ∨ Reach (deps P) p e.1) ∧ (e.2 = q ∨ Reach (deps P) e.2 q)

def negReachB (P : Program) (p q : Nat) : Bool :=
  (negEdges P).any (fun e => (p == e.1 || reachB (deps P) p e.1) && (e.2 == q || reachB (deps P) e.2 q))

theorem negReachB_iff {P : Program} {p q : Nat} : negReachB P p q = true ↔ NegReach P p q := by
  simp only [negReachB, NegReach, List.any_eq_true, Bool.and_eq_true, Bool.or_eq_true,
    beq_iff_eq, reachB_iff]

/-- Every predicate named by an edge. -/
def preds (P : Program) : List Nat := (deps P).flatMap (fun e => [e.1, e.2])

/-- The stratum of `p`: how many predicates `p` reaches through negation. -/
def stratum (P : Program) (p : Nat) : Nat := (preds P).countP (fun q => negReachB P p q)

theorem negReach_of_edge {P : Program} {e : Nat × Nat} (he : e ∈ deps P) {q : Nat}
    (h : NegReach P e.2 q) : NegReach P e.1 q := by
  obtain ⟨f, hf, hl, hr⟩ := h
  refine ⟨f, hf, Or.inr ?_, hr⟩
  rcases hl with hl | hl
  · rw [← hl]; exact .edge he
  · exact .trans (.edge he) hl

theorem negReach_of_negEdge {P : Program} {e : Nat × Nat} (he : e ∈ negEdges P) :
    NegReach P e.1 e.2 :=
  ⟨e, he, Or.inl rfl, Or.inl rfl⟩

theorem not_negReach_self {P : Program} (h : ¬ NegCycle P) (q : Nat) : ¬ NegReach P q q := by
  rintro ⟨e, he, hl, hr⟩
  apply h
  refine ⟨e, he, ?_⟩
  rcases hr with hr | hr <;> rcases hl with hl | hl
  · exact Or.inl (hr.trans hl)
  · rw [hr]; exact Or.inr hl
  · rw [hl] at hr; exact Or.inr hr
  · exact Or.inr (.trans hr hl)

theorem mem_preds_right {P : Program} {e : Nat × Nat} (he : e ∈ deps P) : e.2 ∈ preds P := by
  simp only [preds, List.mem_flatMap, List.mem_cons, List.not_mem_nil, or_false]
  exact ⟨e, he, Or.inr rfl⟩

/-- PO-STRAT, soundness: an accepted program is stratified by `stratum`. -/
theorem stratum_valid {P : Program} (h : stratB P = true) : IsStrat P (stratum P) := by
  have hnc := (stratB_iff_not_negCycle P).mp h
  constructor
  · intro e he
    apply List.countP_mono_left
    intro q _ hq
    exact negReachB_iff.mpr
      (negReach_of_edge (List.mem_append_left _ he) (negReachB_iff.mp hq))
  · intro e he
    apply MetaCore.Store.countP_lt_of_imp
    · intro q _ hq
      exact negReachB_iff.mpr
        (negReach_of_edge (List.mem_append_right _ he) (negReachB_iff.mp hq))
    · refine ⟨e.2, mem_preds_right (List.mem_append_right _ he),
        negReachB_iff.mpr (negReach_of_negEdge he), ?_⟩
      cases hb : negReachB P e.2 e.2
      · rfl
      · exact absurd (negReachB_iff.mp hb) (not_negReach_self hnc e.2)

/-- Along any dependency path a valid stratification never increases. -/
theorem IsStrat.reach_le {P : Program} {s : Nat → Nat} (hs : IsStrat P s) {x y : Nat}
    (h : Reach (deps P) x y) : s y ≤ s x := by
  induction h with
  | edge he =>
    rcases List.mem_append.mp he with he | he
    · exact hs.1 _ he
    · exact Nat.le_of_lt (hs.2 _ he)
  | trans _ _ ih₁ ih₂ => exact Nat.le_trans ih₂ ih₁

/-- PO-STRAT, completeness: a program that has any valid stratification is
accepted. -/
theorem stratB_complete {P : Program} {s : Nat → Nat} (hs : IsStrat P s) : stratB P = true := by
  rw [stratB_iff_not_negCycle]
  rintro ⟨e, he, hc⟩
  have hlt := hs.2 e he
  rcases hc with hc | hc
  · rw [hc] at hlt; exact Nat.lt_irrefl _ hlt
  · exact Nat.lt_irrefl _ (Nat.lt_of_lt_of_le hlt (hs.reach_le hc))

/-- PO-STRAT: the check accepts exactly the programs that have a valid
stratification, i.e. exactly those with no cycle through negation. -/
theorem stratB_iff (P : Program) : stratB P = true ↔ ∃ s, IsStrat P s :=
  ⟨fun h => ⟨stratum P, stratum_valid h⟩, fun ⟨_, hs⟩ => stratB_complete hs⟩

theorem not_negCycle_iff_strat (P : Program) : ¬ NegCycle P ↔ ∃ s, IsStrat P s := by
  rw [← stratB_iff_not_negCycle, stratB_iff]

/-- In a stratified program a negated atom names a strictly lower stratum and
a positive atom a stratum no higher than the rule's head. -/
theorem IsStrat.neg_lower {P : Program} {s : Nat → Nat} (hs : IsStrat P s) {r : Rule}
    (hr : r ∈ P) {a : Atom} (ha : a ∈ r.neg) : s a.pred < s r.head.pred :=
  hs.2 (r.head.pred, a.pred) (List.mem_flatMap.mpr ⟨r, hr, List.mem_map.mpr ⟨a, ha, rfl⟩⟩)

theorem IsStrat.pos_le {P : Program} {s : Nat → Nat} (hs : IsStrat P s) {r : Rule}
    (hr : r ∈ P) {a : Atom} (ha : a ∈ r.pos) : s a.pred ≤ s r.head.pred :=
  hs.1 (r.head.pred, a.pred) (List.mem_flatMap.mpr ⟨r, hr, List.mem_map.mpr ⟨a, ha, rfl⟩⟩)

/-! ## Ground programs: naive least fixpoint -/

/-- A ground rule: head fact and positive body facts. Negated atoms are already
decided against the complete lower strata when grounding (`ground`). -/
abbrev GRule := Fact × List Fact
abbrev GProgram := List GRule

/-- The ground rule's body holds in `I`. -/
def holdsB (I : DB) (g : GRule) : Bool := g.2.all I.contains

/-- Immediate consequences `T_P(I)`. -/
def tp (G : GProgram) (I : DB) : DB := (G.filter (holdsB I)).map Prod.fst

/-- The facts of `N` not yet in `I`. -/
def fresh (I N : DB) : DB := N.filter (fun f => !I.contains f)

/-- The finite Herbrand base of the stratum: every ground head. -/
def hb (G : GProgram) : DB := G.map Prod.fst

def naiveIter (G : GProgram) : Nat → DB → DB
  | 0, I => I
  | n + 1, I => if (fresh I (tp G I)).isEmpty then I else naiveIter G n (I ++ fresh I (tp G I))

/-- Naive evaluation from the base facts `I0`; `|HB| + 1` rounds always suffice
(`naive_closed`). -/
def naive (G : GProgram) (I0 : DB) : DB := naiveIter G ((hb G).length + 1) I0

/-- The least fixpoint, as an inductive set. -/
inductive Derivable (G : GProgram) (I0 : DB) : Fact → Prop
  | base {f : Fact} : f ∈ I0 → Derivable G I0 f
  | rule {g : GRule} : g ∈ G → (∀ b ∈ g.2, Derivable G I0 b) → Derivable G I0 g.1

/-- `J` is closed under the ground rules. -/
def Closed (G : GProgram) (J : DB) : Prop := ∀ g ∈ G, (∀ b ∈ g.2, b ∈ J) → g.1 ∈ J

theorem mem_tp {G : GProgram} {I : DB} {f : Fact} :
    f ∈ tp G I ↔ ∃ g ∈ G, (∀ b ∈ g.2, b ∈ I) ∧ g.1 = f := by
  simp only [tp, holdsB, List.mem_map, List.mem_filter, List.all_eq_true, List.contains_iff_mem,
    and_assoc]

theorem mem_fresh {I N : DB} {f : Fact} : f ∈ fresh I N ↔ f ∈ N ∧ f ∉ I := by
  simp [fresh]

theorem fresh_nil_closed {G : GProgram} {I : DB} (h : fresh I (tp G I) = []) : Closed G I := by
  intro g hg hb
  by_cases hm : g.1 ∈ I
  · exact hm
  · have : g.1 ∈ fresh I (tp G I) := mem_fresh.mpr ⟨mem_tp.mpr ⟨g, hg, hb, rfl⟩, hm⟩
    rw [h] at this; cases this

/-- Ground heads outside `I`: the termination measure. -/
def outside (G : GProgram) (I : DB) : Nat := (hb G).countP (fun f => !I.contains f)

theorem outside_lt {G : GProgram} {I N : DB} (hN : ∀ f ∈ N, f ∈ hb G) (hne : fresh I N ≠ []) :
    outside G (I ++ fresh I N) < outside G I := by
  unfold outside
  apply MetaCore.Store.countP_lt_of_imp
  · intro x _ hx
    simp only [Bool.not_eq_true'] at hx ⊢
    cases hc : I.contains x
    · rfl
    · have : (I ++ fresh I N).contains x = true := by
        rw [List.contains_iff_mem] at hc ⊢; exact List.mem_append_left _ hc
      rw [this] at hx; cases hx
  · obtain ⟨f, hf⟩ := List.exists_mem_of_ne_nil _ hne
    have hf' := mem_fresh.mp hf
    refine ⟨f, hN f hf'.1, ?_, ?_⟩
    · simpa [List.contains_iff_mem] using hf'.2
    · simp [hf]

theorem tp_sub_hb {G : GProgram} {I : DB} : ∀ f ∈ tp G I, f ∈ hb G := by
  intro f hf
  obtain ⟨g, hg, _, rfl⟩ := mem_tp.mp hf
  exact List.mem_map_of_mem hg

theorem naiveIter_closed (G : GProgram) : ∀ (n : Nat) (I : DB),
    outside G I < n → Closed G (naiveIter G n I)
  | 0, _, h => absurd h (Nat.not_lt_zero _)
  | n + 1, I, h => by
    unfold naiveIter
    split
    · rename_i he; exact fresh_nil_closed (List.isEmpty_iff.mp he)
    · rename_i he
      have hne : fresh I (tp G I) ≠ [] := fun h' => he (List.isEmpty_iff.mpr h')
      exact naiveIter_closed G n _ (by have := outside_lt tp_sub_hb hne; omega)

theorem naiveIter_base (G : GProgram) : ∀ (n : Nat) (I : DB), ∀ f ∈ I, f ∈ naiveIter G n I
  | 0, _, f, hf => hf
  | n + 1, I, f, hf => by
    unfold naiveIter
    split
    · exact hf
    · exact naiveIter_base G n _ f (List.mem_append_left _ hf)

theorem naiveIter_sound (G : GProgram) (I0 : DB) : ∀ (n : Nat) (I : DB),
    (∀ f ∈ I, Derivable G I0 f) → ∀ f ∈ naiveIter G n I, Derivable G I0 f
  | 0, _, h => h
  | n + 1, I, h => by
    unfold naiveIter
    split
    · exact h
    · apply naiveIter_sound G I0 n
      intro f hf
      rcases List.mem_append.mp hf with hf | hf
      · exact h f hf
      · obtain ⟨g, hg, hb, rfl⟩ := mem_tp.mp (mem_fresh.mp hf).1
        exact .rule hg (fun b hb' => h b (hb b hb'))

/-- The naive result is closed under the rules (the fuel is enough). -/
theorem naive_closed (G : GProgram) (I0 : DB) : Closed G (naive G I0) :=
  naiveIter_closed G _ _ (Nat.lt_succ_of_le List.countP_le_length)

theorem naive_base (G : GProgram) (I0 : DB) : ∀ f ∈ I0, f ∈ naive G I0 :=
  naiveIter_base G _ _

theorem Derivable.mem_closed {G : GProgram} {I0 J : DB} (hb : ∀ f ∈ I0, f ∈ J)
    (hc : Closed G J) {f : Fact} (h : Derivable G I0 f) : f ∈ J := by
  induction h with
  | base hf => exact hb _ hf
  | rule hg _ ih => exact hc _ hg ih

/-- The naive result is exactly the least fixpoint. -/
theorem mem_naive_iff {G : GProgram} {I0 : DB} {f : Fact} :
    f ∈ naive G I0 ↔ Derivable G I0 f :=
  ⟨fun h => naiveIter_sound G I0 _ _ (fun _ hf => .base hf) f h,
   fun h => h.mem_closed (naive_base G I0) (naive_closed G I0)⟩

/-- The naive result is contained in every closed superset of the base facts. -/
theorem naive_least {G : GProgram} {I0 J : DB} (hb : ∀ f ∈ I0, f ∈ J) (hc : Closed G J) :
    ∀ f ∈ naive G I0, f ∈ J :=
  fun _ hf => (mem_naive_iff.mp hf).mem_closed hb hc

/-! ## Semi-naive evaluation (rule `dl-semi-naive`) -/

/-- One semi-naive round fires only the ground rules whose body holds in `I`
and has at least one atom in the previous delta `Δ`. -/
def deltaFire (G : GProgram) (I Δ : DB) : DB :=
  (G.filter (fun g => holdsB I g && g.2.any Δ.contains)).map Prod.fst

def snStep (G : GProgram) (s : DB × DB) : DB × DB :=
  (s.1 ++ fresh s.1 (deltaFire G s.1 s.2), fresh s.1 (deltaFire G s.1 s.2))

def snIter (G : GProgram) : Nat → DB × DB → DB
  | 0, s => s.1
  | n + 1, s => if s.2.isEmpty then s.1 else snIter G n (snStep G s)

/-- Semi-naive evaluation: one naive round, then delta rounds until the delta
is empty; `|HB| + 2` rounds always suffice. -/
def semiNaive (G : GProgram) (I0 : DB) : DB :=
  snIter G ((hb G).length + 2) (I0 ++ fresh I0 (tp G I0), fresh I0 (tp G I0))

theorem mem_deltaFire {G : GProgram} {I Δ : DB} {f : Fact} :
    f ∈ deltaFire G I Δ ↔ ∃ g ∈ G, ((∀ b ∈ g.2, b ∈ I) ∧ ∃ b ∈ g.2, b ∈ Δ) ∧ g.1 = f := by
  simp only [deltaFire, holdsB, List.mem_map, List.mem_filter, Bool.and_eq_true, List.all_eq_true,
    List.any_eq_true, List.contains_iff_mem, and_assoc]

/-- The semi-naive invariant: a ground rule whose body holds in `I` either has
a body atom in the delta or has already fired. -/
def SNInv (G : GProgram) (s : DB × DB) : Prop :=
  ∀ g ∈ G, (∀ b ∈ g.2, b ∈ s.1) → (∃ b ∈ g.2, b ∈ s.2) ∨ g.1 ∈ s.1

theorem snInv_init (G : GProgram) (I0 : DB) :
    SNInv G (I0 ++ fresh I0 (tp G I0), fresh I0 (tp G I0)) := by
  intro g hg hb
  by_cases hd : ∃ b ∈ g.2, b ∈ fresh I0 (tp G I0)
  · exact Or.inl hd
  · right
    have hb0 : ∀ b ∈ g.2, b ∈ I0 := by
      intro b hb'
      rcases List.mem_append.mp (hb b hb') with h | h
      · exact h
      · exact absurd ⟨b, hb', h⟩ hd
    by_cases hm : g.1 ∈ I0
    · exact List.mem_append_left _ hm
    · exact List.mem_append_right _ (mem_fresh.mpr ⟨mem_tp.mpr ⟨g, hg, hb0, rfl⟩, hm⟩)

theorem snInv_step {G : GProgram} {s : DB × DB} (h : SNInv G s) : SNInv G (snStep G s) := by
  intro g hg hb
  simp only [snStep] at hb ⊢
  by_cases hd : ∃ b ∈ g.2, b ∈ fresh s.1 (deltaFire G s.1 s.2)
  · exact Or.inl hd
  · right
    have hb1 : ∀ b ∈ g.2, b ∈ s.1 := by
      intro b hb'
      rcases List.mem_append.mp (hb b hb') with h' | h'
      · exact h'
      · exact absurd ⟨b, hb', h'⟩ hd
    by_cases hm : g.1 ∈ s.1
    · exact List.mem_append_left _ hm
    · rcases h g hg hb1 with hΔ | hin
      · exact List.mem_append_right _
          (mem_fresh.mpr ⟨mem_deltaFire.mpr ⟨g, hg, ⟨hb1, hΔ⟩, rfl⟩, hm⟩)
      · exact absurd hin hm

/-- Termination measure of the semi-naive loop. -/
def snMeasure (G : GProgram) (s : DB × DB) : Nat := outside G s.1 + (if s.2 = [] then 0 else 1)

theorem deltaFire_sub_hb {G : GProgram} {I Δ : DB} : ∀ f ∈ deltaFire G I Δ, f ∈ hb G := by
  intro f hf
  obtain ⟨g, hg, _, rfl⟩ := mem_deltaFire.mp hf
  exact List.mem_map_of_mem hg

theorem snMeasure_lt {G : GProgram} {s : DB × DB} (hne : s.2 ≠ []) :
    snMeasure G (snStep G s) < snMeasure G s := by
  unfold snMeasure snStep
  simp only [hne, ite_false]
  by_cases hN : fresh s.1 (deltaFire G s.1 s.2) = []
  · simp only [hN, List.append_nil, ite_true]; omega
  · have := outside_lt deltaFire_sub_hb hN
    split <;> omega

theorem snIter_sound (G : GProgram) (I0 : DB) : ∀ (n : Nat) (s : DB × DB),
    (∀ f ∈ s.1, Derivable G I0 f) → ∀ f ∈ snIter G n s, Derivable G I0 f
  | 0, _, h => h
  | n + 1, s, h => by
    unfold snIter
    split
    · exact h
    · apply snIter_sound G I0 n
      intro f hf
      rcases List.mem_append.mp hf with hf | hf
      · exact h f hf
      · obtain ⟨g, hg, ⟨hb, _⟩, rfl⟩ := mem_deltaFire.mp (mem_fresh.mp hf).1
        exact .rule hg (fun b hb' => h b (hb b hb'))

theorem snIter_base (G : GProgram) : ∀ (n : Nat) (s : DB × DB), ∀ f ∈ s.1, f ∈ snIter G n s
  | 0, _, f, hf => hf
  | n + 1, s, f, hf => by
    unfold snIter
    split
    · exact hf
    · exact snIter_base G n _ f (List.mem_append_left _ hf)

theorem snIter_closed (G : GProgram) : ∀ (n : Nat) (s : DB × DB),
    SNInv G s → snMeasure G s < n → Closed G (snIter G n s)
  | 0, _, _, h => absurd h (Nat.not_lt_zero _)
  | n + 1, s, hi, h => by
    unfold snIter
    split
    · rename_i he
      have he := List.isEmpty_iff.mp he
      intro g hg hb
      rcases hi g hg hb with ⟨b, _, hb'⟩ | hm
      · rw [he] at hb'; cases hb'
      · exact hm
    · rename_i he
      have hne : s.2 ≠ [] := fun h' => he (List.isEmpty_iff.mpr h')
      exact snIter_closed G n _ (snInv_step hi) (by have := snMeasure_lt (G := G) hne; omega)

/-- The semi-naive result is closed under the rules (the fuel is enough). -/
theorem semiNaive_closed (G : GProgram) (I0 : DB) : Closed G (semiNaive G I0) := by
  apply snIter_closed G _ _ (snInv_init G I0)
  have h1 : outside G (I0 ++ fresh I0 (tp G I0)) ≤ (hb G).length := List.countP_le_length
  unfold snMeasure
  split <;> simp only at h1 ⊢ <;> omega

/-- PO-SEMINAIVE: semi-naive evaluation computes exactly the naive least
fixpoint. -/
theorem semiNaive_eq_naive (G : GProgram) (I0 : DB) (f : Fact) :
    f ∈ semiNaive G I0 ↔ f ∈ naive G I0 := by
  rw [mem_naive_iff]
  constructor
  · apply snIter_sound G I0
    intro f hf
    rcases List.mem_append.mp hf with hf | hf
    · exact .base hf
    · obtain ⟨g, hg, hb, rfl⟩ := mem_tp.mp (mem_fresh.mp hf).1
      exact .rule hg (fun b hb' => .base (hb b hb'))
  · intro h
    refine h.mem_closed (fun f hf => ?_) (semiNaive_closed G I0)
    exact snIter_base G _ _ f (List.mem_append_left _ hf)

/-! ## Programs: grounding over the active domain -/

/-- A finite valuation; unbound variables read `0`. -/
abbrev Val := List (Nat × Nat)

def Val.get : Val → Nat → Nat
  | [], _ => 0
  | (k, c) :: σ, v => if v = k then c else Val.get σ v

def Term.eval (ν : Nat → Nat) : Term → Nat
  | .var v => ν v
  | .const c => c

/-- The ground instance of an atom under a total valuation. -/
def Atom.inst (ν : Nat → Nat) (a : Atom) : Fact := (a.pred, a.args.map (Term.eval ν))

/-- Every valuation of `vs` with values in `dom`. -/
def assigns : List Nat → List Nat → List Val
  | [], _ => [[]]
  | v :: vs, dom => (assigns vs dom).flatMap (fun σ => dom.map (fun c => (v, c) :: σ))

/-- The negated atoms are absent from the complete lower strata. -/
def negOK (low : DB) (r : Rule) (ν : Nat → Nat) : Bool := r.neg.all (fun a => !low.contains (a.inst ν))

def groundRule (dom : List Nat) (low : DB) (r : Rule) : GProgram :=
  (assigns r.vars.eraseDups dom).filterMap (fun σ =>
    if negOK low r σ.get then some (r.head.inst σ.get, r.pos.map (Atom.inst σ.get)) else none)

def ground (dom : List Nat) (low : DB) (P : Program) : GProgram := P.flatMap (groundRule dom low)

def Term.consts : Term → List Nat
  | .var _ => []
  | .const c => [c]

def Atom.consts (a : Atom) : List Nat := a.args.flatMap Term.consts

/-- The active domain: head constants of the rules and constants of the base facts. -/
def adom (P : Program) (I0 : DB) : List Nat :=
  (P.flatMap (fun r => r.head.consts) ++ I0.flatMap Prod.snd).eraseDups

/-- Naive evaluation of one stratum `P` from the base facts `I0`, with negation
decided against the complete lower strata `low`. -/
def evalNaive (P : Program) (low I0 : DB) : DB := naive (ground (adom P I0) low P) I0

/-- Semi-naive evaluation of one stratum. -/
def evalSemiNaive (P : Program) (low I0 : DB) : DB := semiNaive (ground (adom P I0) low P) I0

/-- PO-SEMINAIVE for a stratum: semi-naive and naive evaluation agree. -/
theorem evalSemiNaive_eq_evalNaive (P : Program) (low I0 : DB) (f : Fact) :
    f ∈ evalSemiNaive P low I0 ↔ f ∈ evalNaive P low I0 :=
  semiNaive_eq_naive _ _ f

/-- The declarative least model of one stratum, over arbitrary (not
domain-restricted) valuations. -/
inductive PDerivable (P : Program) (low I0 : DB) : Fact → Prop
  | base {f : Fact} : f ∈ I0 → PDerivable P low I0 f
  | rule {r : Rule} {ν : Nat → Nat} : r ∈ P →
      (∀ a ∈ r.pos, PDerivable P low I0 (a.inst ν)) →
      (∀ a ∈ r.neg, a.inst ν ∉ low) →
      PDerivable P low I0 (r.head.inst ν)

theorem assigns_complete (ν : Nat → Nat) (dom : List Nat) : ∀ (vs : List Nat),
    (∀ v ∈ vs, ν v ∈ dom) → ∃ τ ∈ assigns vs dom, ∀ v ∈ vs, τ.get v = ν v
  | [], _ => ⟨[], by simp [assigns], by simp⟩
  | v :: vs, h => by
    obtain ⟨τ, hτ, hag⟩ := assigns_complete ν dom vs (fun w hw => h w (List.mem_cons_of_mem _ hw))
    refine ⟨(v, ν v) :: τ, ?_, ?_⟩
    · simp only [assigns, List.mem_flatMap, List.mem_map]
      exact ⟨τ, hτ, ν v, h v (List.mem_cons_self ..), rfl⟩
    · intro w hw
      simp only [Val.get]
      split
      · rename_i hwv; rw [hwv]
      · rename_i hwv
        rcases List.mem_cons.mp hw with hw | hw
        · exact absurd hw hwv
        · exact hag w hw

theorem Atom.inst_congr {a : Atom} {ν μ : Nat → Nat} (h : ∀ v ∈ a.vars, ν v = μ v) :
    a.inst ν = a.inst μ := by
  unfold Atom.inst
  congr 1
  apply List.map_congr_left
  intro t ht
  cases t with
  | var v => exact h v (List.mem_flatMap.mpr ⟨_, ht, by simp [Term.vars]⟩)
  | const c => rfl

theorem Atom.mem_inst_args {a : Atom} {ν : Nat → Nat} {c : Nat} (hc : c ∈ (a.inst ν).2) :
    (∃ v ∈ a.vars, c = ν v) ∨ c ∈ a.consts := by
  simp only [Atom.inst, List.mem_map] at hc
  obtain ⟨t, ht, rfl⟩ := hc
  cases t with
  | var v => exact Or.inl ⟨v, List.mem_flatMap.mpr ⟨_, ht, by simp [Term.vars]⟩, rfl⟩
  | const c => exact Or.inr (List.mem_flatMap.mpr ⟨_, ht, by simp [Term.consts, Term.eval]⟩)

theorem Atom.val_mem_inst {a : Atom} {ν : Nat → Nat} {v : Nat} (hv : v ∈ a.vars) :
    ν v ∈ (a.inst ν).2 := by
  obtain ⟨t, ht, hvt⟩ := List.mem_flatMap.mp hv
  cases t with
  | var w =>
    simp only [Term.vars, List.mem_cons, List.not_mem_nil, or_false] at hvt
    subst hvt
    exact List.mem_map.mpr ⟨_, ht, rfl⟩
  | const c => simp [Term.vars] at hvt

theorem mem_ground {dom : List Nat} {low : DB} {P : Program} {g : GRule} :
    g ∈ ground dom low P ↔ ∃ r ∈ P, ∃ σ ∈ assigns r.vars.eraseDups dom,
      negOK low r σ.get = true ∧ g = (r.head.inst σ.get, r.pos.map (Atom.inst σ.get)) := by
  simp only [ground, groundRule, List.mem_flatMap, List.mem_filterMap]
  constructor
  · rintro ⟨r, hr, σ, hσ, he⟩
    split at he
    · cases he; exact ⟨r, hr, σ, hσ, by assumption, rfl⟩
    · cases he
  · rintro ⟨r, hr, σ, hσ, hn, rfl⟩
    exact ⟨r, hr, σ, hσ, by simp [hn]⟩

/-- Every derived fact has its arguments in the active domain (needs safety:
a head variable is bound by a positive atom). -/
theorem PDerivable.args_adom {P : Program} {low I0 : DB} (hs : ∀ r ∈ P, Safe r) {f : Fact}
    (h : PDerivable P low I0 f) : ∀ c ∈ f.2, c ∈ adom P I0 := by
  induction h with
  | base hf =>
    intro c hc
    exact List.mem_eraseDups.mpr (List.mem_append_right _ (List.mem_flatMap.mpr ⟨_, hf, hc⟩))
  | @rule r ν hr _ _ ih =>
    intro c hc
    rcases Atom.mem_inst_args hc with ⟨v, hv, rfl⟩ | hc
    · obtain ⟨a, ha, hva⟩ := hs r hr v (Or.inl hv)
      exact ih a ha _ (Atom.val_mem_inst hva)
    · exact List.mem_eraseDups.mpr (List.mem_append_left _ (List.mem_flatMap.mpr ⟨r, hr, hc⟩))

/-- For a safe stratum, grounding over the active domain loses nothing:
naive evaluation computes exactly the declarative least model. -/
theorem mem_evalNaive_iff {P : Program} {low I0 : DB} (hs : ∀ r ∈ P, Safe r) {f : Fact} :
    f ∈ evalNaive P low I0 ↔ PDerivable P low I0 f := by
  unfold evalNaive
  rw [mem_naive_iff]
  constructor
  · intro h
    induction h with
    | base hf => exact .base hf
    | @rule g hg _ ih =>
      obtain ⟨r, hr, σ, _, hn, rfl⟩ := mem_ground.mp hg
      refine .rule hr (fun a ha => ih _ (List.mem_map_of_mem ha)) ?_
      intro a ha hl
      have := List.all_eq_true.mp hn a ha
      simp [hl] at this
  · intro h
    have hdom := fun {f} (h : PDerivable P low I0 f) => h.args_adom hs
    induction h with
    | base hf => exact .base hf
    | @rule r ν hr hpos hneg ih =>
      have hvars : ∀ v ∈ r.vars.eraseDups, ν v ∈ adom P I0 := by
        intro v hv
        have hv := List.mem_eraseDups.mp hv
        have hpv : ∃ a ∈ r.pos, v ∈ a.vars := by
          simp only [Rule.vars, List.mem_append, List.mem_flatMap] at hv
          rcases hv with (hv | hv) | hv
          · exact hs r hr v (Or.inl hv)
          · exact hv
          · exact hs r hr v (Or.inr hv)
        obtain ⟨a, ha, hva⟩ := hpv
        exact hdom (hpos a ha) _ (Atom.val_mem_inst hva)
      obtain ⟨τ, hτ, hag⟩ := assigns_complete ν _ _ hvars
      have hagr : ∀ a, (a = r.head ∨ a ∈ r.pos ∨ a ∈ r.neg) → a.inst τ.get = a.inst ν := by
        intro a ha
        apply Atom.inst_congr
        intro v hv
        apply hag v
        apply List.mem_eraseDups.mpr
        simp only [Rule.vars, List.mem_append, List.mem_flatMap]
        rcases ha with rfl | ha | ha
        · exact Or.inl (Or.inl hv)
        · exact Or.inl (Or.inr ⟨a, ha, hv⟩)
        · exact Or.inr ⟨a, ha, hv⟩
      have hn : negOK low r τ.get = true := by
        apply List.all_eq_true.mpr
        intro a ha
        rw [hagr a (Or.inr (Or.inr ha))]
        simpa [List.contains_iff_mem] using hneg a ha
      have hg := mem_ground.mpr ⟨r, hr, τ, hτ, hn, rfl⟩
      rw [← hagr r.head (Or.inl rfl)]
      refine .rule hg ?_
      intro b hb
      obtain ⟨a, ha, rfl⟩ := List.mem_map.mp hb
      rw [hagr a (Or.inr (Or.inl ha))]
      exact ih a ha

/-- PO-SEMINAIVE for a safe stratum: semi-naive evaluation computes exactly
the declarative least model. -/
theorem mem_evalSemiNaive_iff {P : Program} {low I0 : DB} (hs : ∀ r ∈ P, Safe r) {f : Fact} :
    f ∈ evalSemiNaive P low I0 ↔ PDerivable P low I0 f := by
  rw [evalSemiNaive_eq_evalNaive]; exact mem_evalNaive_iff hs

/-! ## Whole programs: the unique stable model (PO-STRAT) -/

/-- Least model of the positive program obtained by fixing negation against `M`
(the Gelfond–Lifschitz reduct of `P` w.r.t. `M`), over arbitrary valuations. -/
inductive GL (P : Program) (I0 : DB) (M : Fact → Prop) : Fact → Prop
  | base {f : Fact} : f ∈ I0 → GL P I0 M f
  | rule {r : Rule} {ν : Nat → Nat} : r ∈ P →
      (∀ a ∈ r.pos, GL P I0 M (a.inst ν)) →
      (∀ a ∈ r.neg, ¬ M (a.inst ν)) →
      GL P I0 M (r.head.inst ν)

/-- `M` is a stable model of `P` over the base facts `I0`: it is exactly the
least model of its own reduct. For stratified programs this is the standard
declarative characterisation of the perfect model. -/
def Stable (P : Program) (I0 : DB) (M : Fact → Prop) : Prop := ∀ f, M f ↔ GL P I0 M f

/-- A derivation of a fact of stratum `≤ k` only consults negation below `k`. -/
theorem GL.agree {P : Program} {s : Nat → Nat} (hst : IsStrat P s) {I0 : DB}
    {M₁ M₂ : Fact → Prop} {k : Nat} (hag : ∀ g : Fact, s g.1 < k → (M₁ g ↔ M₂ g))
    {f : Fact} (h : GL P I0 M₁ f) : s f.1 ≤ k → GL P I0 M₂ f := by
  induction h with
  | base hf => intro _; exact .base hf
  | @rule r ν hr _ hneg ih =>
    intro hk
    refine .rule hr (fun a ha => ih a ha (Nat.le_trans (hst.pos_le hr ha) hk)) ?_
    intro a ha hM
    exact hneg a ha ((hag _ (Nat.lt_of_lt_of_le (hst.neg_lower hr ha) hk)).mpr hM)

/-- PO-STRAT, uniqueness: a stratified program has at most one stable model. -/
theorem stable_unique {P : Program} {s : Nat → Nat} (hst : IsStrat P s) {I0 : DB}
    {M₁ M₂ : Fact → Prop} (h₁ : Stable P I0 M₁) (h₂ : Stable P I0 M₂) (f : Fact) :
    M₁ f ↔ M₂ f := by
  have key : ∀ k, ∀ g : Fact, s g.1 < k → (M₁ g ↔ M₂ g) := by
    intro k
    induction k with
    | zero => intro g hg; exact absurd hg (Nat.not_lt_zero _)
    | succ k ih =>
      intro g hg
      have hle : s g.1 ≤ k := Nat.le_of_lt_succ hg
      rw [h₁ g, h₂ g]
      exact ⟨fun h => GL.agree hst ih h hle,
             fun h => GL.agree hst (fun x hx => (ih x hx).symm) h hle⟩
  exact key (s f.1 + 1) f (Nat.lt_succ_self _)

/-- Bottom-up evaluation along a stratum assignment `s`: stage `k + 1` runs the
rules with head stratum `k` semi-naively on stage `k`, deciding negation
against stage `k`. -/
def stage (P : Program) (s : Nat → Nat) (edb : DB) : Nat → DB
  | 0 => edb
  | k + 1 => evalSemiNaive (P.filter (fun r => s r.head.pred == k)) (stage P s edb k) (stage P s edb k)

/-- The highest head stratum. -/
def top (P : Program) (s : Nat → Nat) : Nat := (P.map (fun r => s r.head.pred)).foldr max 0

theorem le_top {P : Program} {s : Nat → Nat} {r : Rule} (hr : r ∈ P) : s r.head.pred ≤ top P s := by
  induction P with
  | nil => cases hr
  | cons r' P ih =>
    simp only [top, List.map_cons, List.foldr_cons] at ih ⊢
    rcases List.mem_cons.mp hr with rfl | hr
    · exact Nat.le_max_left _ _
    · exact Nat.le_trans (ih hr) (Nat.le_max_right _ _)

/-- The result of evaluating every stratum along `s`. -/
def evalAlong (P : Program) (s : Nat → Nat) (edb : DB) : DB := stage P s edb (top P s + 1)

/-- Evaluate a whole program along the canonical stratification; `none` if the
program is unsafe or not stratified. -/
def evalProgram (P : Program) (edb : DB) : Option DB :=
  if programSafeB P && stratB P then some (evalAlong P (stratum P) edb) else none

section Stages

variable {P : Program} {s : Nat → Nat} {edb : DB}

theorem mem_stage_succ (hs : ∀ r ∈ P, Safe r) {k : Nat} {f : Fact} :
    f ∈ stage P s edb (k + 1) ↔
      PDerivable (P.filter (fun r => s r.head.pred == k)) (stage P s edb k) (stage P s edb k) f :=
  mem_evalSemiNaive_iff (fun r hr => hs r (List.mem_filter.mp hr).1)

theorem stage_mono (hs : ∀ r ∈ P, Safe r) {j : Nat} {f : Fact} (hf : f ∈ stage P s edb j) :
    ∀ d, f ∈ stage P s edb (j + d)
  | 0 => hf
  | d + 1 => (mem_stage_succ hs).mpr (.base (stage_mono hs hf d))

theorem stage_mono_le (hs : ∀ r ∈ P, Safe r) {j m : Nat} (hjm : j ≤ m) {f : Fact}
    (hf : f ∈ stage P s edb j) : f ∈ stage P s edb m := by
  have := stage_mono hs hf (m - j)
  rwa [Nat.add_sub_cancel' hjm] at this

/-- Stage `k + 1` adds only facts of stratum `k`. -/
theorem stage_new (hs : ∀ r ∈ P, Safe r) {k : Nat} {f : Fact} (hf : f ∈ stage P s edb (k + 1))
    (hn : f ∉ stage P s edb k) : s f.1 = k := by
  cases (mem_stage_succ hs).mp hf with
  | base h => exact absurd h hn
  | rule hr _ _ =>
    have := (List.mem_filter.mp hr).2
    simpa [Atom.inst] using this

/-- A fact of stratum below `j` present at a later stage was already present
at stage `j`. -/
theorem stage_low (hs : ∀ r ∈ P, Safe r) {j : Nat} {f : Fact} (hlt : s f.1 < j) :
    ∀ d, f ∈ stage P s edb (j + d) → f ∈ stage P s edb j
  | 0, h => h
  | d + 1, h => by
    by_cases hm : f ∈ stage P s edb (j + d)
    · exact stage_low hs hlt d hm
    · have : s f.1 = j + d := stage_new hs h hm
      omega

theorem stage_low_le (hs : ∀ r ∈ P, Safe r) {j m : Nat} {f : Fact} (hlt : s f.1 < j)
    (hjm : j ≤ m) (h : f ∈ stage P s edb m) : f ∈ stage P s edb j := by
  apply stage_low hs hlt (m - j)
  rwa [Nat.add_sub_cancel' hjm]

/-- Every fact of a stage up to `T` is derivable in the reduct w.r.t. stage `T`. -/
theorem stage_sound (hs : ∀ r ∈ P, Safe r) (hst : IsStrat P s) (T : Nat) :
    ∀ k, k ≤ T → ∀ f ∈ stage P s edb k, GL P edb (· ∈ stage P s edb T) f
  | 0, _, f, hf => .base hf
  | k + 1, hk, f, hf => by
    have ih := stage_sound hs hst T k (Nat.le_of_succ_le hk)
    have hd := (mem_stage_succ hs).mp hf
    clear hf
    induction hd with
    | base h => exact ih _ h
    | @rule r ν hr _ hneg ihp =>
      have hr' := List.mem_filter.mp hr
      have hrk : s r.head.pred = k := by simpa using hr'.2
      refine .rule hr'.1 ihp ?_
      intro a ha hT
      have hlt : s a.pred < k := hrk ▸ hst.neg_lower hr'.1 ha
      exact hneg a ha (stage_low_le hs hlt (Nat.le_of_succ_le hk) hT)

/-- Every fact derivable in the reduct w.r.t. the final stage is in it. -/
theorem stage_complete (hs : ∀ r ∈ P, Safe r) (hst : IsStrat P s) {f : Fact}
    (h : GL P edb (· ∈ evalAlong P s edb) f) : f ∈ evalAlong P s edb := by
  unfold evalAlong at h ⊢
  induction h with
  | base hf => exact stage_mono_le hs (Nat.zero_le _) hf
  | @rule r ν hr _ hneg ihp =>
    have hk : s r.head.pred + 1 ≤ top P s + 1 := Nat.succ_le_succ (le_top hr)
    apply stage_mono_le hs hk
    apply (mem_stage_succ hs).mpr
    refine .rule (List.mem_filter.mpr ⟨hr, by simp⟩) ?_ ?_
    · intro a ha
      apply (mem_stage_succ hs).mp
      exact stage_low_le hs (Nat.lt_succ_of_le (hst.pos_le hr ha)) hk (ihp a ha)
    · intro a ha hk'
      exact hneg a ha (stage_mono_le hs (Nat.le_of_succ_le hk) hk')

end Stages

/-- PO-STRAT, existence: bottom-up evaluation along any valid stratification
yields a stable model. -/
theorem evalAlong_stable {P : Program} {s : Nat → Nat} (hs : ∀ r ∈ P, Safe r)
    (hst : IsStrat P s) (edb : DB) : Stable P edb (· ∈ evalAlong P s edb) :=
  fun _ => ⟨fun h => stage_sound hs hst _ _ (Nat.le_refl _) _ h, stage_complete hs hst⟩

/-- PO-STRAT: the result does not depend on which valid stratification is used. -/
theorem evalAlong_indep {P : Program} {s s' : Nat → Nat} (hs : ∀ r ∈ P, Safe r)
    (h₁ : IsStrat P s) (h₂ : IsStrat P s') (edb : DB) (f : Fact) :
    f ∈ evalAlong P s edb ↔ f ∈ evalAlong P s' edb :=
  stable_unique h₁ (evalAlong_stable hs h₁ edb) (evalAlong_stable hs h₂ edb) f

/-- PO-STRAT for the executable evaluator: a safe stratified program evaluates
to its unique stable model, which equals the evaluation along any valid
stratification. -/
theorem evalProgram_correct {P : Program} {edb : DB} (hs : ∀ r ∈ P, Safe r)
    (hstr : stratB P = true) : ∃ R, evalProgram P edb = some R ∧ Stable P edb (· ∈ R) ∧
      (∀ M, Stable P edb M → ∀ f, M f ↔ f ∈ R) ∧
      (∀ s, IsStrat P s → ∀ f, f ∈ evalAlong P s edb ↔ f ∈ R) := by
  have hst := stratum_valid hstr
  refine ⟨evalAlong P (stratum P) edb, ?_, evalAlong_stable hs hst edb, ?_, ?_⟩
  · simp [evalProgram, (programSafeB_iff P).mpr hs, hstr]
  · intro M hM f
    exact stable_unique hst hM (evalAlong_stable hs hst edb) f
  · intro s' hs' f
    exact evalAlong_indep hs hs' hst edb f

/-- The evaluator refuses exactly the unsafe or unstratifiable programs. -/
theorem evalProgram_none_iff (P : Program) (edb : DB) :
    evalProgram P edb = none ↔ ¬ ((∀ r ∈ P, Safe r) ∧ ∃ s, IsStrat P s) := by
  rw [← programSafeB_iff, ← stratB_iff]
  unfold evalProgram
  cases programSafeB P <;> cases stratB P <;> simp


/- Sanity checks, decided by the kernel. -/

/-- `path(x,y) :- edge(x,y). path(x,z) :- path(x,y), edge(y,z).` -/
def tcProg : Program :=
  [ { head := ⟨1, [.var 0, .var 1]⟩, pos := [⟨0, [.var 0, .var 1]⟩] },
    { head := ⟨1, [.var 0, .var 2]⟩, pos := [⟨1, [.var 0, .var 1]⟩, ⟨0, [.var 1, .var 2]⟩] } ]

example : (evalProgram tcProg [(0, [1, 2]), (0, [2, 3])]).map (fun I => I.filter (·.1 == 1)) =
    some [(1, [1, 2]), (1, [2, 3]), (1, [1, 3])] := by rfl

/- Adding `unreach(x,y) :- node(x), node(y), not path(x,y).` (pred 3, node is
pred 2) puts `unreach` in a higher stratum than `path`. -/
def unreachRule : Rule :=
  { head := ⟨3, [.var 0, .var 1]⟩, pos := [⟨2, [.var 0]⟩, ⟨2, [.var 1]⟩], neg := [⟨1, [.var 0, .var 1]⟩] }

example : (evalProgram (tcProg ++ [unreachRule]) [(2, [1]), (2, [2]), (0, [1, 2])]).map
      (fun I => I.filter (·.1 == 3)) =
    some [(3, [1, 1]), (3, [2, 1]), (3, [2, 2])] := by rfl

/-- `p(x) :- q(x), not p(x).` has a cycle through negation. -/
example : stratB [{ head := ⟨0, [.var 0]⟩, pos := [⟨1, [.var 0]⟩], neg := [⟨0, [.var 0]⟩] }] = false := by
  rfl

/-- `p(x) :- q(x).` with `x` only in the head of another rule is unsafe. -/
example : safeB { head := ⟨0, [.var 1]⟩, pos := [⟨1, [.var 0]⟩] } = false := by rfl

end MetaCore.Datalog
