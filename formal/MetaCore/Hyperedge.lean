/-
  # Hyperedges (tg.md/08-hyperedges.md, ADR 0010, ADR 0006)

  A hyperedge is an item with an ID from the same allocator as statements (one
  shared counter, `Store.next`), a type, and a list of `(role, filler)` pairs.
  Fillers are nodes or statements, never literals ([[adr:0028]]) and never other
  hyperedges. A type declares its roles (all of cardinality one in this model)
  and which of them are required; a write whose roles do not match the type is
  rejected and allocates nothing (rule `he-type-declared`).

  The store is extended to `HStore` (statements plus hyperedges) and the cascade
  delete of `MetaCore.Store` is extended by the hyperedge guard of rule
  `he-delete-filler`: a statement that fills a role of a hyperedge blocks its
  delete unless `DETACH` is given; with `DETACH` the whole hyperedge is deleted.
  A filler is never nulled. Nothing refers to a hyperedge in this model, so a
  hyperedge can always be deleted by ID.

  Not modelled: annotations of hyperedges, promoted role edges (`hedge_promo`),
  literal attributes, node deletion. The statement-level closure is the one of
  `MetaCore.Store` unchanged, so `closure_closed` (PO-CASCADE-FUEL) carries over.
-/
import MetaCore.Store

namespace MetaCore.Hyperedge

open MetaCore.Store (Term Triple Store Entry ids termIn doomed removeAll RefClosed WF
  blockedB lookup)

/-! ## Types and shape validation (rule `he-type-declared`) -/

/-- A hyperedge type: its declared roles (cardinality one each) and the subset
that is required. -/
structure HType where
  id : Nat
  roles : List Nat
  required : List Nat
  deriving Repr

/-- A role assignment as written: `(role, filler)` pairs in the order given. -/
abbrev Roles := List (Nat × Term)

def roleNames (rs : Roles) : List Nat := rs.map Prod.fst

/-- The roles `rs` satisfy type `T`: every role named is declared, no role is
named twice (all cardinalities are one), and every required role is present. -/
structure Satisfies (T : HType) (rs : Roles) : Prop where
  declared : ∀ r ∈ roleNames rs, r ∈ T.roles
  once : (roleNames rs).Nodup
  required : ∀ r ∈ T.required, r ∈ roleNames rs

/-- The executable shape check. -/
def shapeB (T : HType) (rs : Roles) : Bool :=
  (roleNames rs).all (fun r => T.roles.contains r) &&
  decide (roleNames rs).Nodup &&
  T.required.all (fun r => (roleNames rs).contains r)

theorem shapeB_iff {T : HType} {rs : Roles} : shapeB T rs = true ↔ Satisfies T rs := by
  unfold shapeB
  simp only [Bool.and_eq_true, List.all_eq_true, List.contains_iff_mem, decide_eq_true_eq]
  constructor
  · rintro ⟨⟨h₁, h₂⟩, h₃⟩; exact ⟨h₁, h₂, h₃⟩
  · rintro ⟨h₁, h₂, h₃⟩; exact ⟨⟨h₁, h₂⟩, h₃⟩

/-- A filler is a node or a live statement; a literal is never a filler. -/
def fillerOk (st : Store) : Term → Bool
  | .node _ => true
  | .stmt i => (ids st).contains i
  | .lit _ => false

/-- Validation of a hyperedge write against the type table `types`. -/
def validB (types : Nat → Option HType) (st : Store) (ty : Nat) (rs : Roles) : Bool :=
  match types ty with
  | none => false
  | some T => shapeB T rs && rs.all (fun rf => fillerOk st rf.2)

/-- Arity: a hyperedge that satisfies its type names at most as many roles as
the type declares. -/
theorem satisfies_arity {T : HType} {rs : Roles} (h : Satisfies T rs) :
    rs.length ≤ T.roles.length := by
  have := List.Nodup.length_le_of_subset h.once (fun r hr => h.declared r hr)
  simpa [roleNames] using this

/-- Rule `he-type-fixed`: when every declared role is required, a hyperedge that
satisfies the type has exactly the type's arity. -/
theorem satisfies_fixed_arity {T : HType} {rs : Roles} (h : Satisfies T rs)
    (hT : T.roles.Nodup) (hall : ∀ r ∈ T.roles, r ∈ T.required) :
    rs.length = T.roles.length := by
  have h₁ := satisfies_arity h
  have h₂ := List.Nodup.length_le_of_subset hT (fun r hr => h.required r (hall r hr))
  simp [roleNames] at h₂
  omega

/-! ## The extended store -/

structure HEntry where
  id : Nat
  ty : Nat
  /-- The graph the hyperedge was created in (recorded only). -/
  graph : Nat
  roles : Roles
  deriving Repr

/-- Statements and hyperedges. The allocator is `st.next`, shared by both. -/
structure HStore where
  st : Store
  hedges : List HEntry
  deriving Repr

def hids (hs : HStore) : List Nat := hs.hedges.map HEntry.id

def empty : HStore := ⟨Store.empty, []⟩

section
variable (multi : Nat → Bool) (types : Nat → Option HType)

/-- Well-formedness of the extended store: the statement invariant `WF`, a
positive allocator, hyperedge IDs positive, below the allocator, distinct and
disjoint from statement IDs, every hyperedge of a declared type it satisfies,
and no literal filler. -/
structure HWF (hs : HStore) : Prop where
  wf : WF multi hs.st
  next_pos : 0 < hs.st.next
  pos : ∀ h ∈ hs.hedges, 0 < h.id
  lt_next : ∀ h ∈ hs.hedges, h.id < hs.st.next
  nodup : (hids hs).Nodup
  disjoint : ∀ h ∈ hs.hedges, h.id ∉ ids hs.st
  shape : ∀ h ∈ hs.hedges, ∃ T, types h.ty = some T ∧ Satisfies T h.roles
  noLit : ∀ h ∈ hs.hedges, ∀ rf ∈ h.roles, ∀ n, rf.2 ≠ .lit n

/-- No hyperedge has a missing statement as a filler. -/
def HedgeRefClosed (hs : HStore) : Prop :=
  ∀ h ∈ hs.hedges, ∀ rf ∈ h.roles, ∀ i, rf.2 = .stmt i → i ∈ ids hs.st

/-- No dangling reference anywhere: statements (subject and object) and roles. -/
def HRefClosed (hs : HStore) : Prop := RefClosed hs.st ∧ HedgeRefClosed hs

theorem empty_hwf : HWF multi types empty where
  wf := MetaCore.Store.empty_wf multi
  next_pos := by decide
  pos := by simp [empty]
  lt_next := by simp [empty]
  nodup := by simp [empty, hids]
  disjoint := by simp [empty]
  shape := by simp [empty]
  noLit := by simp [empty]

theorem empty_hrefClosed : HRefClosed empty :=
  ⟨by simp [RefClosed, empty, MetaCore.Store.empty], by simp [HedgeRefClosed, empty]⟩

/-! ## Statement writes on the extended store -/

def insert (hs : HStore) (t : Triple) : HStore × Nat :=
  let r := MetaCore.Store.insert multi hs.st t
  ({ hs with st := r.1 }, r.2)

def update (hs : HStore) (id : Nat) (o : Term) : Option HStore :=
  (MetaCore.Store.update multi hs.st id o).map (fun st' => { hs with st := st' })

/-- Insert either returns an existing ID and changes nothing, or prepends a fresh
entry at the old allocator mark. -/
theorem store_insert_cases (st : Store) (t : Triple) :
    (MetaCore.Store.insert multi st t = (st, (MetaCore.Store.insert multi st t).2)) ∨
    (MetaCore.Store.insert multi st t =
      ({ stmts := (st.next, t) :: st.stmts, next := st.next + 1 }, st.next)) := by
  unfold MetaCore.Store.insert
  split
  · split
    · exact Or.inl rfl
    · exact Or.inr rfl
  · exact Or.inr rfl

/-- Insert only adds the returned ID. -/
theorem store_insert_ids (st : Store) (t : Triple) :
    ∀ j ∈ ids (MetaCore.Store.insert multi st t).1,
      j ∈ ids st ∨ j = (MetaCore.Store.insert multi st t).2 := by
  intro j hj
  rcases store_insert_cases multi st t with h | h <;> rw [h] at hj ⊢
  · exact Or.inl hj
  · simp only [ids, List.map_cons, List.mem_cons] at hj
    rcases hj with rfl | hj
    · exact Or.inr rfl
    · exact Or.inl hj

/-- Insert never loses an ID. -/
theorem store_insert_ids_sub (st : Store) (t : Triple) :
    ∀ j ∈ ids st, j ∈ ids (MetaCore.Store.insert multi st t).1 := by
  intro j hj
  rcases store_insert_cases multi st t with h | h <;> rw [h]
  · exact hj
  · simp only [ids, List.map_cons, List.mem_cons]; exact Or.inr hj

/-- The ID insert returns is live afterwards. -/
theorem store_insert_id_mem (st : Store) (t : Triple) :
    (MetaCore.Store.insert multi st t).2 ∈ ids (MetaCore.Store.insert multi st t).1 := by
  rcases MetaCore.Store.insert_id_fresh_or_existing (multi := multi) (st := st) (t := t)
    with ⟨h₁, h₂⟩ | _
  · rw [h₁]; exact List.mem_map.mpr ⟨_, h₂, rfl⟩
  · rcases store_insert_cases multi st t with h | h <;> rw [h]
    · -- existing case returned `st` unchanged; the ID is in it
      rcases MetaCore.Store.insert_id_fresh_or_existing (multi := multi) (st := st) (t := t)
        with ⟨_, h₂⟩ | ⟨h₃, h₄⟩
      · exact List.mem_map.mpr ⟨_, h₂, rfl⟩
      · -- here `insert` returned `st` and also bumped `next`: impossible
        have : (MetaCore.Store.insert multi st t).1.next = st.next := by rw [h]
        omega
    · simp [ids]

/-- Statement terms of a triple name live statements. -/
def TermsLive (st : Store) (t : Triple) : Prop :=
  ∀ i, (t.s = .stmt i ∨ t.o = .stmt i) → i ∈ ids st

/-- Insert keeps `RefClosed` when the new triple names only live statements
(the precondition the oracle generator maintains). -/
theorem store_insert_refClosed {st : Store} {t : Triple} (hrc : RefClosed st)
    (ht : TermsLive st t) : RefClosed (MetaCore.Store.insert multi st t).1 := by
  intro e he i hs
  apply store_insert_ids_sub multi st t
  rcases store_insert_cases multi st t with h | h <;> rw [h] at he
  · exact hrc e he i hs
  · simp only [List.mem_cons] at he
    rcases he with rfl | he
    · exact ht i hs
    · exact hrc e he i hs

/-- Update keeps `RefClosed` when the new object names only live statements. -/
theorem store_update_refClosed {st st' : Store} {id : Nat} {o : Term} (hrc : RefClosed st)
    (ho : ∀ i, o = .stmt i → i ∈ ids st) (h : MetaCore.Store.update multi st id o = some st') :
    RefClosed st' := by
  have hids := MetaCore.Store.update_ids multi h
  unfold MetaCore.Store.update at h
  split at h
  · cases h
  · rename_i t hl
    dsimp only at h
    split at h
    · cases h
    · cases h
      intro e he i hs
      rw [hids]
      simp only [List.mem_map] at he
      obtain ⟨e₀, he₀, rfl⟩ := he
      unfold MetaCore.Store.replaceObj at hs
      split at hs
      · rcases hs with hs | hs
        · exact hrc _ (MetaCore.Store.lookup_some hl) i (Or.inl hs)
        · exact ho i hs
      · exact hrc e₀ he₀ i hs


/-! ## Hyperedge creation -/

/-- Create a hyperedge of type `ty` in graph `g`. A valid write takes the next ID
from the shared allocator; an invalid one changes nothing. -/
def create (hs : HStore) (ty g : Nat) (rs : Roles) : Option (HStore × Nat) :=
  if validB types hs.st ty rs then
    some ({ st := { hs.st with next := hs.st.next + 1 },
            hedges := ⟨hs.st.next, ty, g, rs⟩ :: hs.hedges }, hs.st.next)
  else none

/-- A rejected hyperedge allocates nothing. -/
theorem create_none_iff {hs : HStore} {ty g : Nat} {rs : Roles} :
    create types hs ty g rs = none ↔ validB types hs.st ty rs = false := by
  unfold create; split <;> simp_all

/-- Scenario "valid hyperedge accepted": an accepted hyperedge has a declared
type that it satisfies (every role declared, each at most once, every required
role present), so its arity is at most the type's, and exactly the type's when
every role is required; it gets the next ID of the shared allocator and no
literal or missing statement fills a role. -/
theorem create_some {hs hs' : HStore} {ty g i : Nat} {rs : Roles}
    (h : create types hs ty g rs = some (hs', i)) :
    i = hs.st.next ∧ hs'.st.next = hs.st.next + 1 ∧ hs'.st.stmts = hs.st.stmts ∧
    hs'.hedges = ⟨i, ty, g, rs⟩ :: hs.hedges ∧
    (∃ T, types ty = some T ∧ Satisfies T rs ∧ rs.length ≤ T.roles.length) ∧
    (∀ rf ∈ rs, fillerOk hs.st rf.2 = true) := by
  unfold create at h
  split at h
  · rename_i hv
    cases h
    refine ⟨rfl, rfl, rfl, rfl, ?_, ?_⟩
    · unfold validB at hv
      split at hv
      · cases hv
      · rename_i T hT
        simp only [Bool.and_eq_true] at hv
        have hs := shapeB_iff.mp hv.1
        exact ⟨T, hT, hs, satisfies_arity hs⟩
    · unfold validB at hv
      split at hv
      · cases hv
      · simp only [Bool.and_eq_true, List.all_eq_true] at hv
        exact hv.2
  · cases h

theorem fillerOk_noLit {st : Store} {f : Term} (h : fillerOk st f = true) : ∀ n, f ≠ .lit n := by
  intro n hf; subst hf; simp [fillerOk] at h

theorem fillerOk_live {st : Store} {f : Term} (h : fillerOk st f = true) :
    ∀ i, f = .stmt i → i ∈ ids st := by
  intro i hf; subst hf; simpa [fillerOk] using h

theorem create_hwf {hs hs' : HStore} {ty g i : Nat} {rs : Roles}
    (hwf : HWF multi types hs) (h : create types hs ty g rs = some (hs', i)) :
    HWF multi types hs' := by
  obtain ⟨rfl, hnext, hstmts, hhed, ⟨T, hT, hsat, _⟩, hfill⟩ := create_some types h
  have hI : ids hs'.st = ids hs.st := by simp [ids, hstmts]
  refine ⟨⟨?_, ?_, ?_, ?_⟩, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro e he; rw [hstmts] at he; exact hwf.wf.pos e he
  · intro e he; rw [hstmts] at he; have := hwf.wf.lt_next e he; omega
  · rw [hI]; exact hwf.wf.nodup
  · intro e₁ h₁ e₂ h₂; rw [hstmts] at h₁ h₂; exact hwf.wf.unique e₁ h₁ e₂ h₂
  · omega
  · intro h' hh; rw [hhed] at hh
    rcases List.mem_cons.mp hh with rfl | hh
    · exact hwf.next_pos
    · exact hwf.pos h' hh
  · intro h' hh; rw [hhed] at hh
    rcases List.mem_cons.mp hh with rfl | hh
    · simp; omega
    · have := hwf.lt_next h' hh; omega
  · simp only [hids, hhed, List.map_cons, List.nodup_cons]
    refine ⟨?_, hwf.nodup⟩
    intro hm
    obtain ⟨h', hh, heq⟩ := List.mem_map.mp hm
    have := hwf.lt_next h' hh; omega
  · intro h' hh; rw [hhed] at hh; rw [hI]
    rcases List.mem_cons.mp hh with rfl | hh
    · intro hm
      obtain ⟨e, he, heq⟩ := List.mem_map.mp hm
      have := hwf.wf.lt_next e he; simp only at heq; omega
    · exact hwf.disjoint h' hh
  · intro h' hh; rw [hhed] at hh
    rcases List.mem_cons.mp hh with rfl | hh
    · exact ⟨T, hT, hsat⟩
    · exact hwf.shape h' hh
  · intro h' hh; rw [hhed] at hh
    rcases List.mem_cons.mp hh with rfl | hh
    · intro rf hrf; exact fillerOk_noLit (hfill rf hrf)
    · exact hwf.noLit h' hh

theorem create_hrefClosed {hs hs' : HStore} {ty g i : Nat} {rs : Roles}
    (hrc : HRefClosed hs) (h : create types hs ty g rs = some (hs', i)) : HRefClosed hs' := by
  obtain ⟨rfl, _, hstmts, hhed, _, hfill⟩ := create_some types h
  have hI : ids hs'.st = ids hs.st := by simp [ids, hstmts]
  refine ⟨?_, ?_⟩
  · intro e he j hj; rw [hI]; rw [hstmts] at he; exact hrc.1 e he j hj
  · intro h' hh rf hrf j hj; rw [hI]; rw [hhed] at hh
    rcases List.mem_cons.mp hh with rfl | hh
    · exact fillerOk_live (hfill rf hrf) j hj
    · exact hrc.2 h' hh rf hrf j hj

/-! ## Cascade delete with the hyperedge guard (rule `he-delete-filler`) -/

/-- Some role of the hyperedge is filled by a statement in `D`. -/
def fills (D : List Nat) (h : HEntry) : Bool := h.roles.any (fun rf => termIn D rf.2)

/-- The hyperedge guard: some hyperedge has a filler in `D`. Hyperedges are
pulled into the doomed set only under `DETACH`, so without it every such
hyperedge survives and the delete is blocked. -/
def hedgeBlockedB (hs : HStore) (D : List Nat) : Bool := hs.hedges.any (fills D)

def removeHedge (hs : HStore) (id : Nat) : HStore :=
  { hs with hedges := hs.hedges.filter (fun h => h.id != id) }

/-- Cascade delete on the extended store. Deleting a hyperedge removes it and
nothing else (nothing refers to a hyperedge). Deleting a statement runs the
statement cascade of `MetaCore.Store.cascadeDelete` (same doomed set `D`, same
object guard) and then the hyperedge guard: without `detach` a hyperedge with a
filler in `D` blocks; with `detach` every such hyperedge is deleted whole. -/
def cascadeDelete (hs : HStore) (id : Nat) (detach : Bool) : Option HStore :=
  if (hids hs).contains id then some (removeHedge hs id)
  else
    let D := doomed hs.st id detach
    match MetaCore.Store.cascadeDelete hs.st id detach with
    | none => none
    | some st' =>
      if !detach && hedgeBlockedB hs D then none
      else some { st := st', hedges := hs.hedges.filter (fun h => !fills D h) }

theorem fills_of_mem {D : List Nat} {h : HEntry} {rf : Nat × Term} {i : Nat}
    (hrf : rf ∈ h.roles) (hi : rf.2 = .stmt i) (hD : i ∈ D) : fills D h = true := by
  unfold fills
  rw [List.any_eq_true]
  exact ⟨rf, hrf, MetaCore.Store.termIn_stmt hi hD⟩

/-- A hyperedge can always be deleted by ID, never blocked, and only it goes. -/
theorem cascadeDelete_hedge {hs : HStore} {id : Nat} {detach : Bool} (h : id ∈ hids hs) :
    cascadeDelete hs id detach = some (removeHedge hs id) := by
  unfold cascadeDelete
  simp [h]

/-- On a statement, exactly the statement cascade's doomed set `D` is removed
from the statements, and exactly the hyperedges with a filler in `D` are removed
(none of them under a successful plain delete). Surviving hyperedges are
unchanged: a filler is never nulled. -/
theorem cascadeDelete_removed {hs hs' : HStore} {id : Nat} {detach : Bool}
    (hn : id ∉ hids hs) (h : cascadeDelete hs id detach = some hs') :
    hs'.st = removeAll hs.st (doomed hs.st id detach) ∧
    hs'.hedges = hs.hedges.filter (fun h => !fills (doomed hs.st id detach) h) := by
  unfold cascadeDelete at h
  simp only [List.contains_iff_mem, hn, ite_false] at h
  split at h
  · cases h
  · rename_i st' hst
    split at h
    · cases h
    · cases h
      exact ⟨MetaCore.Store.cascadeDelete_removed hst, rfl⟩

/-- Without `DETACH`, a successful statement delete removes no hyperedge. -/
theorem cascadeDelete_plain_keeps_hedges {hs hs' : HStore} {id : Nat}
    (hn : id ∉ hids hs) (h : cascadeDelete hs id false = some hs') :
    hs'.hedges = hs.hedges := by
  unfold cascadeDelete at h
  simp only [List.contains_iff_mem, hn, ite_false] at h
  split at h
  · cases h
  · split at h
    · cases h
    · rename_i hb
      cases h
      simp only [Bool.not_false, Bool.true_and, hedgeBlockedB, List.any_eq_true,
        not_exists, not_and, Bool.not_eq_true] at hb
      simp only
      apply List.filter_eq_self.mpr
      intro x hx; simp [hb x hx]

theorem cascadeDelete_hedges_sub {hs hs' : HStore} {id : Nat} {detach : Bool}
    (h : cascadeDelete hs id detach = some hs') : ∀ x ∈ hs'.hedges, x ∈ hs.hedges := by
  intro x hx
  by_cases hm : id ∈ hids hs
  · rw [cascadeDelete_hedge hm] at h; cases h
    exact (List.mem_filter.mp hx).1
  · rw [(cascadeDelete_removed hm h).2] at hx
    exact (List.mem_filter.mp hx).1

theorem cascadeDelete_ids_sub {hs hs' : HStore} {id : Nat} {detach : Bool}
    (h : cascadeDelete hs id detach = some hs') : ∀ j ∈ ids hs'.st, j ∈ ids hs.st := by
  intro j hj
  by_cases hm : id ∈ hids hs
  · rw [cascadeDelete_hedge hm] at h; cases h; exact hj
  · rw [(cascadeDelete_removed hm h).1] at hj
    simp only [ids, removeAll, List.mem_map, List.mem_filter] at hj ⊢
    obtain ⟨e, ⟨he, _⟩, rfl⟩ := hj
    exact ⟨e, he, rfl⟩

theorem cascadeDelete_next {hs hs' : HStore} {id : Nat} {detach : Bool}
    (h : cascadeDelete hs id detach = some hs') : hs'.st.next = hs.st.next := by
  by_cases hm : id ∈ hids hs
  · rw [cascadeDelete_hedge hm] at h; cases h; rfl
  · rw [(cascadeDelete_removed hm h).1]; rfl

/-- A delete fails exactly when the target is a statement and either the object
guard (ADR 0036) or, without `DETACH`, the hyperedge guard fires. -/
theorem cascadeDelete_none_iff {hs : HStore} {id : Nat} {detach : Bool} :
    cascadeDelete hs id detach = none ↔
      id ∉ hids hs ∧ (blockedB hs.st (doomed hs.st id detach) = true ∨
        (detach = false ∧ hedgeBlockedB hs (doomed hs.st id detach) = true)) := by
  by_cases hm : id ∈ hids hs
  · rw [cascadeDelete_hedge hm]; simp [hm]
  · unfold cascadeDelete
    simp only [List.contains_iff_mem, hm, ite_false, not_false_eq_true, true_and]
    cases hc : MetaCore.Store.cascadeDelete hs.st id detach with
    | none =>
      have := MetaCore.Store.cascadeDelete_none_iff.mp hc
      simp [this]
    | some st' =>
      have hb : blockedB hs.st (doomed hs.st id detach) = false := by
        cases hb : blockedB hs.st (doomed hs.st id detach)
        · rfl
        · have := MetaCore.Store.cascadeDelete_none_iff.mpr hb; rw [hc] at this; cases this
      simp only [hb, Bool.false_eq_true, false_or]
      cases detach <;> cases hh : hedgeBlockedB hs (doomed hs.st id _) <;> simp

/-- `DETACH` is never blocked, by the object guard or by the hyperedge guard. -/
theorem cascadeDelete_detach_some (hs : HStore) (id : Nat) :
    cascadeDelete hs id true ≠ none := by
  intro h
  obtain ⟨_, hb⟩ := cascadeDelete_none_iff.mp h
  rcases hb with hb | ⟨hd, _⟩
  · have := MetaCore.Store.cascadeDelete_none_iff.mpr hb
    exact MetaCore.Store.cascadeDelete_detach_some hs.st id this
  · cases hd

/-- Guard necessity for roles: if a plain delete is blocked by the hyperedge
guard, some hyperedge has a doomed statement as a filler, so removing the doomed
statements while keeping the hyperedges (the only option, since a filler is
never nulled) would leave a dangling filler. -/
theorem cascadeDelete_hedge_blocked {hs : HStore} {id : Nat}
    (hb : hedgeBlockedB hs (doomed hs.st id false) = true) :
    (∃ h ∈ hs.hedges, ∃ rf ∈ h.roles, ∃ i, rf.2 = .stmt i ∧ i ∈ doomed hs.st id false) ∧
    ¬ HedgeRefClosed { st := removeAll hs.st (doomed hs.st id false), hedges := hs.hedges } := by
  unfold hedgeBlockedB fills at hb
  simp only [List.any_eq_true] at hb
  obtain ⟨h, hh, rf, hrf, ht⟩ := hb
  cases hf : rf.2 with
  | node _ => simp [termIn, hf] at ht
  | lit _ => simp [termIn, hf] at ht
  | stmt i =>
    have hi : i ∈ doomed hs.st id false := by simpa [termIn, hf] using ht
    refine ⟨⟨h, hh, rf, hrf, i, hf, hi⟩, ?_⟩
    intro hrc
    exact MetaCore.Store.removeAll_gone hi (hrc h hh rf hrf i hf)

/-- Scenario "Delete with and without DETACH" (rule `he-delete-filler` plus
ADR 0006/0036): a successful cascade on the extended store keeps `HWF`, leaves
no dangling subject, object or role filler (`HRefClosed`), and the deleted ID is
gone, whether it named a statement or a hyperedge. -/
theorem cascadeDelete_correct {hs hs' : HStore} {id : Nat} {detach : Bool}
    (hwf : HWF multi types hs) (hrc : HRefClosed hs) (h : cascadeDelete hs id detach = some hs') :
    HWF multi types hs' ∧ HRefClosed hs' ∧ id ∉ ids hs'.st ∧ id ∉ hids hs' := by
  have hsub := cascadeDelete_hedges_sub h
  have hidsub := cascadeDelete_ids_sub h
  have hnext := cascadeDelete_next h
  -- the statement part
  have hst : WF multi hs'.st ∧ RefClosed hs'.st ∧ (id ∉ hids hs → id ∉ ids hs'.st) := by
    by_cases hm : id ∈ hids hs
    · rw [cascadeDelete_hedge hm] at h; cases h
      exact ⟨hwf.wf, hrc.1, fun hn => absurd hm hn⟩
    · have hr := cascadeDelete_removed hm h
      have hsc : MetaCore.Store.cascadeDelete hs.st id detach = some hs'.st := by
        unfold cascadeDelete at h
        simp only [List.contains_iff_mem, hm, ite_false] at h
        split at h
        · cases h
        · rename_i st' hst'
          split at h
          · cases h
          · cases h; exact hst'
      obtain ⟨a, b, c⟩ := MetaCore.Store.cascadeDelete_correct multi hwf.wf hrc.1 hsc
      exact ⟨a, b, fun _ => c⟩
  refine ⟨⟨hst.1, by rw [hnext]; exact hwf.next_pos, fun x hx => hwf.pos x (hsub x hx),
    fun x hx => by rw [hnext]; exact hwf.lt_next x (hsub x hx), ?_,
    fun x hx hm => hwf.disjoint x (hsub x hx) (hidsub _ hm),
    fun x hx => hwf.shape x (hsub x hx), fun x hx => hwf.noLit x (hsub x hx)⟩,
    ⟨hst.2.1, ?_⟩, ?_, ?_⟩
  · -- hyperedge IDs stay distinct: the new list is a filter of the old one
    by_cases hm : id ∈ hids hs
    · rw [cascadeDelete_hedge hm] at h; cases h
      exact (List.filter_sublist.map HEntry.id).nodup hwf.nodup
    · rw [hids, (cascadeDelete_removed hm h).2]
      exact (List.filter_sublist.map HEntry.id).nodup hwf.nodup
  · -- no surviving hyperedge has a deleted filler
    intro x hx rf hrf i hi
    by_cases hm : id ∈ hids hs
    · rw [cascadeDelete_hedge hm] at h; cases h
      exact hrc.2 x (hsub x hx) rf hrf i hi
    · obtain ⟨hr₁, hr₂⟩ := cascadeDelete_removed hm h
      rw [hr₂] at hx
      have hx' := List.mem_filter.mp hx
      have hiD : i ∉ doomed hs.st id detach := by
        intro hiD
        have := fills_of_mem hrf hi hiD
        simp [this] at hx'
      have hlive := hrc.2 x hx'.1 rf hrf i hi
      rw [hr₁]
      simp only [ids, List.mem_map] at hlive ⊢
      obtain ⟨e, he, rfl⟩ := hlive
      exact ⟨e, by simp [removeAll, List.mem_filter, he, hiD], rfl⟩
  · by_cases hm : id ∈ hids hs
    · obtain ⟨x, hx, hxid⟩ := List.mem_map.mp hm
      intro hin
      exact hwf.disjoint x hx (hxid ▸ hidsub _ hin)
    · exact hst.2.2 hm
  · by_cases hm : id ∈ hids hs
    · rw [cascadeDelete_hedge hm] at h; cases h
      simp [hids, removeHedge, List.mem_filter]
    · intro hin
      obtain ⟨x, hx, rfl⟩ := List.mem_map.mp hin
      exact hm (List.mem_map.mpr ⟨x, hsub x hx, rfl⟩)

theorem hids_def (hs : HStore) : hids hs = hs.hedges.map HEntry.id := rfl

/-! ## Statement writes keep the extended invariants -/

theorem insert_hwf {hs : HStore} {t : Triple} (hwf : HWF multi types hs) :
    HWF multi types (insert multi hs t).1 := by
  have hpos := hwf.next_pos
  have hmono := MetaCore.Store.insert_next_mono multi hs.st t
  refine ⟨MetaCore.Store.insert_wf multi hwf.wf hpos, by simp [insert]; omega,
    hwf.pos, fun x hx => by have := hwf.lt_next x hx; simp [insert]; omega, hwf.nodup,
    ?_, hwf.shape, hwf.noLit⟩
  intro x hx hm
  rcases store_insert_ids multi hs.st t x.id hm with hm | hm
  · exact hwf.disjoint x hx hm
  · rcases MetaCore.Store.insert_id_fresh_or_existing (multi := multi) (st := hs.st) (t := t)
      with ⟨_, h₂⟩ | ⟨h₂, _⟩
    · exact hwf.disjoint x hx (hm ▸ List.mem_map.mpr ⟨_, h₂, rfl⟩)
    · have := hwf.lt_next x hx; omega

theorem insert_hrefClosed {hs : HStore} {t : Triple} (hrc : HRefClosed hs)
    (ht : TermsLive hs.st t) : HRefClosed (insert multi hs t).1 :=
  ⟨store_insert_refClosed multi hrc.1 ht,
   fun x hx rf hrf i hi => store_insert_ids_sub multi hs.st t i (hrc.2 x hx rf hrf i hi)⟩

theorem update_hwf {hs hs' : HStore} {id : Nat} {o : Term} (hwf : HWF multi types hs)
    (h : update multi hs id o = some hs') : HWF multi types hs' := by
  unfold update at h
  cases hu : MetaCore.Store.update multi hs.st id o with
  | none => rw [hu] at h; cases h
  | some st' =>
    rw [hu] at h; cases h
    have hids := MetaCore.Store.update_ids multi hu
    have hnext := MetaCore.Store.update_next multi hu
    exact ⟨MetaCore.Store.update_wf multi hwf.wf hu, by simp [hnext]; exact hwf.next_pos,
      hwf.pos, fun x hx => by simp [hnext]; exact hwf.lt_next x hx, hwf.nodup,
      fun x hx => by simp [hids]; exact hwf.disjoint x hx, hwf.shape, hwf.noLit⟩

theorem update_hrefClosed {hs hs' : HStore} {id : Nat} {o : Term} (hrc : HRefClosed hs)
    (ho : ∀ i, o = .stmt i → i ∈ ids hs.st) (h : update multi hs id o = some hs') :
    HRefClosed hs' := by
  unfold update at h
  cases hu : MetaCore.Store.update multi hs.st id o with
  | none => rw [hu] at h; cases h
  | some st' =>
    rw [hu] at h; cases h
    have hids := MetaCore.Store.update_ids multi hu
    exact ⟨store_update_refClosed multi hrc.1 ho hu,
      fun x hx rf hrf i hi => by simp only [hids]; exact hrc.2 x hx rf hrf i hi⟩

theorem update_ids' {hs hs' : HStore} {id : Nat} {o : Term}
    (h : update multi hs id o = some hs') : ids hs'.st = ids hs.st ∧ hs'.hedges = hs.hedges ∧
      hs'.st.next = hs.st.next := by
  unfold update at h
  cases hu : MetaCore.Store.update multi hs.st id o with
  | none => rw [hu] at h; cases h
  | some st' =>
    rw [hu] at h; cases h
    exact ⟨MetaCore.Store.update_ids multi hu, rfl, MetaCore.Store.update_next multi hu⟩

end

end MetaCore.Hyperedge
