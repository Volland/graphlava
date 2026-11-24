/-
  # Graph membership, nesting and scope (tg.md/09-graphs.md, ADR 0007, ADR 0008, ADR 0024)

  The whole observable world of the oracle: the extended store of
  `MetaCore.Hyperedge`, the direct graph memberships of every statement, and the
  nesting edges between named graphs. Graph 0 is the default graph. One
  statement has one ID and a set of direct graphs (dataset semantics, ADR 0024):

  * insert into g of an existing non-multi triple adds membership g and returns
    the existing ID (rule `gr-insert-existing`);
  * removing the last membership deletes the statement, as a plain (non-`DETACH`)
    cascade delete, which may be blocked (rule `gr-delete-in-graph`);
  * nesting is guarded by the cycle check of `MetaCore.Nesting.addEdge_acyclic`
    (rule `gr-dag-cycle`), decided by an executable reachability closure proved
    equal to `Reach`;
  * scope is the direct members of g, or of g and every graph nested under it
    (rules `gr-scope-transitive`, `gr-scope-direct-mode`).

  `apply` is the single step function the oracle (`Main.lean`) runs; the
  invariants below are stated over it, so they hold for every generated trace.
  Not modelled: home graphs and chunks, `moved` tombstones, promoted
  memberships, partitioned graphs, graph drop, the depth limit.
-/
import MetaCore.Hyperedge
import MetaCore.Nesting

namespace MetaCore.Graphs

open MetaCore.Store (Term Triple Store ids RefClosed WF)
open MetaCore.Hyperedge (HType HEntry HStore Roles hids HWF HRefClosed TermsLive)
open MetaCore.Nesting (Edges Reach Acyclic InScope)

/-! ## Executable reachability (the `in_star` check) -/

/-- One round: add the target of every edge leaving `a` or a member of `S`. -/
def rstep (E : Edges) (a : Nat) (S : List Nat) : List Nat :=
  S ++ (E.filter (fun e => (e.1 == a || S.contains e.1) && !S.contains e.2)).map Prod.snd

def rclosure (E : Edges) (a : Nat) : Nat → List Nat → List Nat
  | 0, S => S
  | n + 1, S => if (rstep E a S).length = S.length then S else rclosure E a n (rstep E a S)

/-- Every graph reachable from `a` by one or more edges. -/
def reachable (E : Edges) (a : Nat) : List Nat := rclosure E a (E.length + 1) []

/-- `reachB E a b`: `a` is nested (transitively) under `b`. -/
def reachB (E : Edges) (a b : Nat) : Bool := (reachable E a).contains b

def RClosed (E : Edges) (a : Nat) (S : List Nat) : Prop :=
  ∀ e ∈ E, (e.1 = a ∨ e.1 ∈ S) → e.2 ∈ S

theorem rstep_sound {E : Edges} {a : Nat} {S : List Nat} (h : ∀ x ∈ S, Reach E a x) :
    ∀ x ∈ rstep E a S, Reach E a x := by
  intro x hx
  simp only [rstep, List.mem_append, List.mem_map, List.mem_filter] at hx
  rcases hx with hx | ⟨e, ⟨he, hc⟩, rfl⟩
  · exact h x hx
  · simp only [Bool.and_eq_true, Bool.or_eq_true, beq_iff_eq, List.contains_iff_mem] at hc
    rcases hc.1 with h1 | h1
    · exact .edge (by rw [← h1]; exact he)
    · exact .trans (h _ h1) (.edge he)

theorem rclosure_sound (E : Edges) (a : Nat) : ∀ (n : Nat) (S : List Nat),
    (∀ x ∈ S, Reach E a x) → ∀ x ∈ rclosure E a n S, Reach E a x
  | 0, _, h => h
  | n + 1, S, h => by
    unfold rclosure
    split
    · exact h
    · exact rclosure_sound E a n _ (rstep_sound h)

/-- Entries whose target is not yet in `S`. -/
def routside (E : Edges) (S : List Nat) : Nat := E.countP (fun e => !S.contains e.2)

theorem rstep_fixpoint {E : Edges} {a : Nat} {S : List Nat}
    (h : (rstep E a S).length = S.length) : RClosed E a S := by
  intro e he hc
  simp only [rstep, List.length_append, List.length_map] at h
  have hnil := List.eq_nil_of_length_eq_zero (by omega :
    (E.filter (fun e => (e.1 == a || S.contains e.1) && !S.contains e.2)).length = 0)
  rw [List.filter_eq_nil_iff] at hnil
  have := hnil e he
  by_cases hn : e.2 ∈ S
  · exact hn
  · exfalso
    apply this
    rcases hc with hc | hc
    · simp [hc, hn]
    · simp [hc, hn]

theorem routside_lt {E : Edges} {a : Nat} {S : List Nat}
    (h : (rstep E a S).length ≠ S.length) : routside E (rstep E a S) < routside E S := by
  unfold routside
  apply MetaCore.Store.countP_lt_of_imp
  · intro x _ hx
    cases hS : S.contains x.2
    · rfl
    · exfalso
      have : (rstep E a S).contains x.2 = true := by
        simp only [rstep, List.contains_iff_mem, List.mem_append] at hS ⊢
        exact Or.inl hS
      rw [this] at hx; cases hx
  · have hne : (E.filter (fun e => (e.1 == a || S.contains e.1) && !S.contains e.2)) ≠ [] := by
      intro hnil; apply h; simp only [rstep, hnil, List.map_nil, List.append_nil]
    obtain ⟨e, he⟩ := List.exists_mem_of_ne_nil _ hne
    have he' := List.mem_filter.mp he
    refine ⟨e, he'.1, ?_, ?_⟩
    · have := he'.2
      simp only [Bool.and_eq_true] at this
      exact this.2
    · simp only [rstep, Bool.not_eq_false']
      simp only [List.contains_iff_mem, List.mem_append, List.mem_map]
      exact Or.inr ⟨e, he, rfl⟩

theorem rclosure_closed (E : Edges) (a : Nat) : ∀ (n : Nat) (S : List Nat),
    routside E S < n → RClosed E a (rclosure E a n S)
  | 0, _, h => absurd h (Nat.not_lt_zero _)
  | n + 1, S, h => by
    unfold rclosure
    split
    · rename_i hfix; exact rstep_fixpoint hfix
    · rename_i hne
      exact rclosure_closed E a n _ (by have := routside_lt hne; omega)

theorem reachable_closed (E : Edges) (a : Nat) : RClosed E a (reachable E a) :=
  rclosure_closed E a _ _ (Nat.lt_succ_of_le List.countP_le_length)

theorem closed_complete {E : Edges} {a : Nat} {S : List Nat} (hc : RClosed E a S) :
    ∀ {x y : Nat}, Reach E x y → (x = a ∨ x ∈ S) → y ∈ S := by
  intro x y h
  induction h with
  | edge he => intro hx; exact hc _ he hx
  | trans _ _ ih₁ ih₂ => intro hx; exact ih₂ (Or.inr (ih₁ hx))

/-- The executable check decides `Reach` exactly (fuel `|E| + 1` always reaches
the fixpoint, as for the cascade closure). -/
theorem reachB_iff {E : Edges} {a b : Nat} : reachB E a b = true ↔ Reach E a b := by
  unfold reachB
  rw [List.contains_iff_mem]
  constructor
  · intro h
    exact rclosure_sound E a _ [] (by simp) b h
  · intro h
    exact closed_complete (reachable_closed E a) h (Or.inl rfl)

/-! ## The world -/

structure World where
  hs : HStore
  /-- Direct graph memberships per item ID. Only live statements are read. -/
  mem : Nat → List Nat
  /-- Nesting edges `(child, parent)` between named graphs. -/
  nest : Edges

def empty : World := ⟨MetaCore.Hyperedge.empty, fun _ => [], []⟩

/-- Point update of the membership map. -/
def upd (m : Nat → List Nat) (k : Nat) (v : List Nat) : Nat → List Nat :=
  fun j => if j = k then v else m j

/-- Add graph `g` to a membership list (no-op if present). -/
def addG (g : Nat) (l : List Nat) : List Nat := if l.contains g then l else l ++ [g]

/-- Every live statement is a direct member of at least one graph, and its
membership list has no duplicates. -/
def MemOK (w : World) : Prop := ∀ i ∈ ids w.hs.st, w.mem i ≠ [] ∧ (w.mem i).Nodup

/-- All live item IDs, statements and hyperedges. -/
def allIds (hs : HStore) : List Nat := ids hs.st ++ hids hs

/-- The sorted IDs present before and missing after. -/
def gone (hs hs' : HStore) : List Nat :=
  ((allIds hs).filter (fun i => !(allIds hs').contains i)).mergeSort (· ≤ ·)

/-- Graph `h` is in the scope of `g`: `h = g` or `h` is nested under `g`. -/
def inScopeB (E : Edges) (g h : Nat) : Bool := h == g || reachB E h g

/-- Rules `gr-scope-transitive` and `gr-scope-direct-mode`: the sorted live
statement IDs whose direct graphs include `g` (direct) or any graph in the
scope of `g` (transitive). Each ID once. -/
def scope (w : World) (g : Nat) (direct : Bool) : List Nat :=
  ((ids w.hs.st).filter (fun i =>
      if direct then (w.mem i).contains g else (w.mem i).any (inScopeB w.nest g))).mergeSort
    (· ≤ ·)

inductive Op where
  | insert (t : Triple) (g : Nat)
  | update (id : Nat) (o : Term)
  | delete (id : Nat) (detach : Bool)
  | hedge (ty g : Nat) (roles : Roles)
  | addMember (id g : Nat)
  | removeMember (id g : Nat)
  | nest (child parent : Nat)
  | unnest (child parent : Nat)
  | scope (g : Nat) (direct : Bool)

/-- The observable result of one operation. -/
inductive Res where
  | id (i : Nat)
  | ok (b : Bool)
  /-- Success with the sorted list of removed IDs. -/
  | removed (l : List Nat)
  /-- Failure because a guard (object or hyperedge) blocked the delete. -/
  | blocked
  | ids (l : List Nat)

section
variable (multi : Nat → Bool) (types : Nat → Option HType)

def apply (w : World) : Op → World × Res
  | .insert t g =>
    let r := MetaCore.Hyperedge.insert multi w.hs t
    let l := if (ids w.hs.st).contains r.2 then addG g (w.mem r.2) else [g]
    ({ w with hs := r.1, mem := upd w.mem r.2 l }, .id r.2)
  | .update id o =>
    match MetaCore.Hyperedge.update multi w.hs id o with
    | some hs' => ({ w with hs := hs' }, .ok true)
    | none => (w, .ok false)
  | .delete id detach =>
    if (allIds w.hs).contains id then
      match MetaCore.Hyperedge.cascadeDelete w.hs id detach with
      | some hs' => ({ w with hs := hs' }, .removed (gone w.hs hs'))
      | none => (w, .blocked)
    else (w, .ok false)
  | .hedge ty g rs =>
    match MetaCore.Hyperedge.create types w.hs ty g rs with
    | some (hs', i) => ({ w with hs := hs' }, .id i)
    | none => (w, .ok false)
  | .addMember id g =>
    if (ids w.hs.st).contains id then
      ({ w with mem := upd w.mem id (addG g (w.mem id)) }, .ok true)
    else (w, .ok false)
  | .removeMember id g =>
    if (ids w.hs.st).contains id && (w.mem id).contains g then
      let l := (w.mem id).filter (· != g)
      if l.isEmpty then
        -- last membership: a plain cascade delete of the statement
        match MetaCore.Hyperedge.cascadeDelete w.hs id false with
        | some hs' => ({ w with hs := hs' }, .removed (gone w.hs hs'))
        | none => (w, .blocked)
      else ({ w with mem := upd w.mem id l }, .removed [])
    else (w, .ok false)
  | .nest c p =>
    if c == p || reachB w.nest p c then (w, .ok false)
    else if w.nest.contains (c, p) then (w, .ok true)
    else ({ w with nest := (c, p) :: w.nest }, .ok true)
  | .unnest c p =>
    if w.nest.contains (c, p) then ({ w with nest := w.nest.filter (· != (c, p)) }, .ok true)
    else (w, .ok false)
  | .scope g direct => (w, .ids (scope w g direct))

/-! ## Invariants over every operation -/

theorem addG_ne_nil (g : Nat) (l : List Nat) : addG g l ≠ [] := by
  unfold addG; split
  · rename_i h; intro hl; subst hl; simp at h
  · simp

theorem addG_nodup {g : Nat} {l : List Nat} (h : l.Nodup) : (addG g l).Nodup := by
  unfold addG; split
  · exact h
  · rename_i hg
    rw [List.nodup_append]
    refine ⟨h, by simp, ?_⟩
    intro a ha b hb hab
    simp at hb; subst hb; subst hab
    exact hg (by simpa using ha)

theorem addG_mem (g : Nat) (l : List Nat) : g ∈ addG g l := by
  unfold addG; split
  · rename_i h; simpa using h
  · simp

theorem newMem_ok {m : Nat → List Nat} {L : List Nat} (h : ∀ i ∈ L, m i ≠ [] ∧ (m i).Nodup)
    (x g : Nat) :
    (if L.contains x = true then addG g (m x) else [g]) ≠ [] ∧
    (if L.contains x = true then addG g (m x) else [g]).Nodup := by
  split
  · rename_i hc
    exact ⟨addG_ne_nil _ _, addG_nodup (h _ (by simpa using hc)).2⟩
  · exact ⟨by simp, by simp⟩

theorem newMem_mem (m : Nat → List Nat) (L : List Nat) (x g : Nat) :
    g ∈ (if L.contains x = true then addG g (m x) else [g]) := by
  split
  · exact addG_mem _ _
  · simp

/-- The membership invariant behind "the statement is deleted when its last
membership is removed": after any operation every live statement is still a
direct member of at least one graph, with no duplicate memberships. -/
theorem apply_memOK {w : World} (op : Op) (h : MemOK w) : MemOK (apply multi types w op).1 := by
  cases op with
  | insert t g =>
    intro i hi
    simp only [apply, upd] at hi ⊢
    split
    · exact newMem_ok h _ _
    · rename_i hne
      have hi' : i ∈ ids (MetaCore.Store.insert multi w.hs.st t).1 := hi
      rcases MetaCore.Hyperedge.store_insert_ids multi w.hs.st t i hi' with hi | hi
      · exact h i hi
      · exact absurd hi hne
  | update id o =>
    simp only [apply]
    split
    · rename_i hs' hu
      intro i hi
      have := (MetaCore.Hyperedge.update_ids' multi hu).1
      rw [this] at hi
      exact h i hi
    · exact h
  | delete id detach =>
    simp only [apply]
    split
    · split
      · rename_i hs' hd
        intro i hi
        exact h i (MetaCore.Hyperedge.cascadeDelete_ids_sub hd i hi)
      · exact h
    · exact h
  | hedge ty g rs =>
    simp only [apply]
    split
    · rename_i hs' i' hc
      intro i hi
      obtain ⟨_, _, hstmts, _⟩ := MetaCore.Hyperedge.create_some types hc
      have : ids hs'.st = ids w.hs.st := by simp [ids, hstmts]
      rw [this] at hi
      exact h i hi
    · exact h
  | addMember id g =>
    simp only [apply]
    split
    · intro i hi
      simp only [upd]
      split
      · subst_vars; exact ⟨addG_ne_nil _ _, addG_nodup (h _ hi).2⟩
      · exact h i hi
    · exact h
  | removeMember id g =>
    simp only [apply]
    split
    · rename_i hc
      simp only [Bool.and_eq_true, List.contains_iff_mem] at hc
      split
      · split
        · rename_i hs' hd
          intro i hi
          exact h i (MetaCore.Hyperedge.cascadeDelete_ids_sub hd i hi)
        · exact h
      · rename_i hne
        intro i hi
        simp only [upd]
        split
        · subst_vars
          refine ⟨by simpa using hne, ?_⟩
          exact (List.filter_sublist).nodup (h _ hi).2
        · exact h i hi
    · exact h
  | nest c p =>
    simp only [apply]
    split
    · exact h
    · split
      · exact h
      · exact h
  | unnest c p =>
    simp only [apply]
    split
    · exact h
    · exact h
  | scope g d => exact h

/-- Rule `gr-dag-cycle` across the whole step function: nesting stays acyclic
under every operation, in particular nest (guarded by `addEdge_acyclic`) and
unnest (removing an edge cannot create a cycle). -/
theorem apply_acyclic {w : World} (op : Op) (h : Acyclic w.nest) :
    Acyclic (apply multi types w op).1.nest := by
  cases op with
  | insert t g => exact h
  | update id o =>
    simp only [apply]; split <;> exact h
  | delete id detach =>
    simp only [apply]; split
    · split <;> exact h
    · exact h
  | hedge ty g rs =>
    simp only [apply]; split <;> exact h
  | addMember id g =>
    simp only [apply]; split <;> exact h
  | removeMember id g =>
    simp only [apply]; split
    · split
      · split <;> exact h
      · exact h
    · exact h
  | nest c p =>
    simp only [apply]
    split
    · exact h
    · rename_i hg
      simp only [Bool.or_eq_true, beq_iff_eq, not_or] at hg
      split
      · exact h
      · exact MetaCore.Nesting.addEdge_acyclic h hg.1 (fun hr => hg.2 (reachB_iff.mpr hr))
  | unnest c p =>
    simp only [apply]
    split
    · intro g hg
      exact h g (hg.mono (fun x hx => (List.mem_filter.mp hx).1))
    · exact h
  | scope g d => exact h

/-- Nest is rejected exactly when the edge would close a cycle: `c = p` or `p`
is already nested under `c`. Otherwise it succeeds and the edge is present. -/
theorem apply_nest {w : World} {c p : Nat} :
    ((apply multi types w (.nest c p)).2 = .ok false ↔ (c = p ∨ Reach w.nest p c)) ∧
    ((apply multi types w (.nest c p)).2 = .ok true →
      (c, p) ∈ (apply multi types w (.nest c p)).1.nest) := by
  simp only [apply]
  constructor
  · split
    · rename_i hg
      simp only [Bool.or_eq_true, beq_iff_eq, reachB_iff] at hg
      simp [hg]
    · rename_i hg
      simp only [Bool.or_eq_true, beq_iff_eq, reachB_iff, not_or] at hg
      split <;> simp [hg]
  · split
    · simp
    · split
      · rename_i hm; intro; simpa using hm
      · intro; simp

/-- Scope is exactly the live statements with a direct graph in the scope of `g`
(`InScope`, rule `gr-scope-transitive`), or with `g` itself as a direct graph in
direct mode (`gr-scope-direct-mode`). -/
theorem scope_mem {w : World} {g i : Nat} {direct : Bool} :
    i ∈ scope w g direct ↔ i ∈ ids w.hs.st ∧
      (if direct then g ∈ w.mem i else ∃ h ∈ w.mem i, InScope w.nest g h) := by
  unfold scope
  rw [List.mem_mergeSort, List.mem_filter]
  cases direct
  · simp only [Bool.false_eq_true, ite_false, List.any_eq_true, inScopeB, Bool.or_eq_true,
      beq_iff_eq, reachB_iff, InScope]
  · simp

/-- Scenario "Shared child counts once": each ID appears once in a scope result,
even when it is reachable through several graphs or paths. -/
theorem scope_nodup {w : World} {g : Nat} {direct : Bool} (hwf : WF multi w.hs.st) :
    (scope w g direct).Nodup :=
  ((List.mergeSort_perm _ _).nodup_iff).mpr ((List.filter_sublist).nodup hwf.nodup)

/-- Scope is sorted ascending. -/
theorem scope_sorted (w : World) (g : Nat) (direct : Bool) :
    (scope w g direct).Pairwise (fun a b => decide (a ≤ b) = true) :=
  List.pairwise_mergeSort (by intro a b c h₁ h₂; simp at *; omega)
    (by intro a b; simp; omega) _

/-- Rule `gr-insert-existing`: inserting into g leaves the returned ID a live
direct member of g; on a non-multi predicate an existing triple keeps its ID. -/
theorem apply_insert_member (w : World) (t : Triple) (g : Nat) :
    let r := apply multi types w (.insert t g)
    ∃ i, r.2 = .id i ∧ i ∈ ids r.1.hs.st ∧ g ∈ r.1.mem i := by
  refine ⟨_, rfl, ?_, ?_⟩
  · exact MetaCore.Hyperedge.store_insert_id_mem multi w.hs.st t
  · simp only [apply, upd, ite_true]
    exact newMem_mem _ _ _ _

/-- Rule `gr-delete-in-graph`: removing a membership that is not the last one
keeps the statement and drops only that membership; removing the last one either
deletes the statement (a plain cascade) or is blocked and changes nothing. -/
theorem apply_removeMember {w : World} {id g : Nat}
    (hl : id ∈ ids w.hs.st) (hg : g ∈ w.mem id) :
    let r := apply multi types w (.removeMember id g)
    ((w.mem id).filter (· != g) ≠ [] →
      r.1.hs = w.hs ∧ r.1.mem id = (w.mem id).filter (· != g) ∧ r.2 = .removed []) ∧
    ((w.mem id).filter (· != g) = [] →
      (r.1 = w ∧ r.2 = .blocked ∧ MetaCore.Hyperedge.cascadeDelete w.hs id false = none) ∨
      (MetaCore.Hyperedge.cascadeDelete w.hs id false = some r.1.hs ∧ r.1.mem = w.mem)) := by
  have hc : ((ids w.hs.st).contains id && (w.mem id).contains g) = true := by
    simp [hl, hg]
  refine ⟨?_, ?_⟩
  · intro hne
    simp only [apply, hc, ite_true]
    have : ((w.mem id).filter (· != g)).isEmpty = false := by
      cases hf : (w.mem id).filter (· != g) <;> simp_all
    simp [this, upd]
  · intro he
    simp only [apply, hc, ite_true]
    have : ((w.mem id).filter (· != g)).isEmpty = true := by simp [he]
    simp only [this, ite_true]
    cases hd : MetaCore.Hyperedge.cascadeDelete w.hs id false with
    | none => exact Or.inl ⟨rfl, rfl, rfl⟩
    | some hs' => exact Or.inr ⟨rfl, rfl⟩

/-- The extended store invariant `HWF` holds after every operation. -/
theorem apply_hwf {w : World} (op : Op) (hwf : HWF multi types w.hs) (hrc : HRefClosed w.hs) :
    HWF multi types (apply multi types w op).1.hs := by
  cases op with
  | insert t g => exact MetaCore.Hyperedge.insert_hwf multi types hwf
  | update id o =>
    simp only [apply]; split
    · rename_i hs' hu; exact MetaCore.Hyperedge.update_hwf multi types hwf hu
    · exact hwf
  | delete id detach =>
    simp only [apply]; split
    · split
      · rename_i hs' hd; exact (MetaCore.Hyperedge.cascadeDelete_correct multi types hwf hrc hd).1
      · exact hwf
    · exact hwf
  | hedge ty g rs =>
    simp only [apply]; split
    · rename_i hs' i hc; exact MetaCore.Hyperedge.create_hwf multi types hwf hc
    · exact hwf
  | addMember id g => simp only [apply]; split <;> exact hwf
  | removeMember id g =>
    simp only [apply]; split
    · split
      · split
        · rename_i hs' hd
          exact (MetaCore.Hyperedge.cascadeDelete_correct multi types hwf hrc hd).1
        · exact hwf
      · exact hwf
    · exact hwf
  | nest c p => simp only [apply]; split
                · exact hwf
                · split <;> exact hwf
  | unnest c p => simp only [apply]; split <;> exact hwf
  | scope g d => exact hwf

/-- The operation names only live statements in term positions (what the
generator guarantees; hyperedge fillers are validated by `create` itself). -/
def OpLive (w : World) : Op → Prop
  | .insert t _ => TermsLive w.hs.st t
  | .update _ o => ∀ i, o = .stmt i → i ∈ ids w.hs.st
  | _ => True

/-- No dangling statement or role reference after any operation whose terms
name live statements. -/
theorem apply_hrefClosed {w : World} (op : Op) (hwf : HWF multi types w.hs)
    (hrc : HRefClosed w.hs) (hl : OpLive w op) : HRefClosed (apply multi types w op).1.hs := by
  cases op with
  | insert t g => exact MetaCore.Hyperedge.insert_hrefClosed multi hrc hl
  | update id o =>
    simp only [apply]; split
    · rename_i hs' hu; exact MetaCore.Hyperedge.update_hrefClosed multi hrc hl hu
    · exact hrc
  | delete id detach =>
    simp only [apply]; split
    · split
      · rename_i hs' hd
        exact (MetaCore.Hyperedge.cascadeDelete_correct multi types hwf hrc hd).2.1
      · exact hrc
    · exact hrc
  | hedge ty g rs =>
    simp only [apply]; split
    · rename_i hs' i hc; exact MetaCore.Hyperedge.create_hrefClosed types hrc hc
    · exact hrc
  | addMember id g => simp only [apply]; split <;> exact hrc
  | removeMember id g =>
    simp only [apply]; split
    · split
      · split
        · rename_i hs' hd
          exact (MetaCore.Hyperedge.cascadeDelete_correct multi types hwf hrc hd).2.1
        · exact hrc
      · exact hrc
    · exact hrc
  | nest c p => simp only [apply]; split
                · exact hrc
                · split <;> exact hrc
  | unnest c p => simp only [apply]; split <;> exact hrc
  | scope g d => exact hrc

/-- Rule `id-never-reused` across every operation, hyperedge creation included:
statements and hyperedges share one allocator and it never moves backwards. -/
theorem apply_next_mono {w : World} (op : Op) :
    w.hs.st.next ≤ (apply multi types w op).1.hs.st.next := by
  cases op with
  | insert t g => exact MetaCore.Store.insert_next_mono multi w.hs.st t
  | update id o =>
    simp only [apply]; split
    · rename_i hs' hu; rw [(MetaCore.Hyperedge.update_ids' multi hu).2.2]; exact Nat.le_refl _
    · exact Nat.le_refl _
  | delete id detach =>
    simp only [apply]; split
    · split
      · rename_i hs' hd; rw [MetaCore.Hyperedge.cascadeDelete_next hd]; exact Nat.le_refl _
      · exact Nat.le_refl _
    · exact Nat.le_refl _
  | hedge ty g rs =>
    simp only [apply]; split
    · rename_i hs' i hc
      rw [(MetaCore.Hyperedge.create_some types hc).2.1]; omega
    · exact Nat.le_refl _
  | addMember id g => simp only [apply]; split <;> exact Nat.le_refl _
  | removeMember id g =>
    simp only [apply]; split
    · split
      · split
        · rename_i hs' hd; rw [MetaCore.Hyperedge.cascadeDelete_next hd]; exact Nat.le_refl _
        · exact Nat.le_refl _
      · exact Nat.le_refl _
    · exact Nat.le_refl _
  | nest c p => simp only [apply]; split
                · exact Nat.le_refl _
                · split <;> exact Nat.le_refl _
  | unnest c p => simp only [apply]; split <;> exact Nat.le_refl _
  | scope g d => exact Nat.le_refl _

/-- The full oracle invariant. -/
structure Inv (w : World) : Prop where
  hwf : HWF multi types w.hs
  hrc : HRefClosed w.hs
  mem : MemOK w
  acyclic : Acyclic w.nest

theorem empty_inv : Inv multi types empty where
  hwf := MetaCore.Hyperedge.empty_hwf multi types
  hrc := MetaCore.Hyperedge.empty_hrefClosed
  mem := by simp [MemOK, empty, MetaCore.Hyperedge.empty, MetaCore.Store.empty, ids]
  acyclic := by
    intro g hg
    cases hg with
    | edge he => simp [empty] at he
    | trans h₁ _ =>
      exfalso
      have : ∀ a b, Reach ([] : Edges) a b → False := by
        intro a b h; induction h with
        | edge he => simp at he
        | trans _ _ ih => exact ih
      exact this _ _ h₁

/-- Every operation of the oracle trace keeps the full invariant, given that the
generator only names live statements in term positions. -/
theorem apply_inv {w : World} (op : Op) (h : Inv multi types w) (hl : OpLive w op) :
    Inv multi types (apply multi types w op).1 :=
  ⟨apply_hwf multi types op h.hwf h.hrc, apply_hrefClosed multi types op h.hwf h.hrc hl,
   apply_memOK multi types op h.mem, apply_acyclic multi types op h.acyclic⟩

end

end MetaCore.Graphs
