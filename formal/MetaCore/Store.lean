/-
  # Statement store (tg.md/01-data-model.md, ADR 0004, ADR 0006)

  An abstract, executable model of the statement store: statements with stable
  IDs, unique `(s,p,o)` unless the predicate is `multi`, update in place, and
  cascade delete of annotations. Storage layout (orderings, inline property
  records) is deliberately absent: this is the *observable* state every layout
  must refine.

  Not modelled here (covered by tests, see tg.md/19-verification.md):
  inline-property routing and promotion, keep-annotations delete. The
  object-position guard of ADR 0036 is modelled; the hyperedge guard on delete
  is modelled on the extended store in `MetaCore.Hyperedge`.
-/
namespace MetaCore.Store

/-- A term in subject/object position. `stmt id` is a statement used as a term,
which is how statements about statements are expressed. -/
inductive Term where
  | node (n : Nat)
  | lit (n : Nat)
  | stmt (id : Nat)
  deriving DecidableEq, Repr

structure Triple where
  s : Term
  p : Nat
  o : Term
  deriving DecidableEq, Repr

abbrev Entry := Nat × Triple

structure Store where
  stmts : List Entry
  /-- Allocator high-water mark. ID 0 is the null ID and never allocated. -/
  next : Nat
  deriving Repr

def ids (st : Store) : List Nat := st.stmts.map Prod.fst

def lookup (st : Store) (id : Nat) : Option Triple :=
  (st.stmts.find? (fun e => e.1 == id)).map Prod.snd

def findTriple (st : Store) (t : Triple) : Option Nat :=
  (st.stmts.find? (fun e => e.2 == t)).map Prod.fst

def empty : Store := ⟨[], 1⟩

section
-- The meta-model's `multi` flag per predicate (rule `dm-multi-predicate`). -/
variable (multi : Nat → Bool)

/-- Well-formedness: the invariants every reachable store satisfies. -/
structure WF (st : Store) : Prop where
  pos : ∀ e ∈ st.stmts, 0 < e.1
  lt_next : ∀ e ∈ st.stmts, e.1 < st.next
  nodup : (ids st).Nodup
  /-- Rule `dm-unique-spo`. -/
  unique : ∀ e₁ ∈ st.stmts, ∀ e₂ ∈ st.stmts,
    e₁.2 = e₂.2 → multi e₁.2.p = false → e₁.1 = e₂.1

theorem empty_wf : WF multi empty where
  pos := by simp [empty]
  lt_next := by simp [empty]
  nodup := by simp [empty, ids]
  unique := by simp [empty]

/-! ## Insert -/

def fresh (st : Store) (t : Triple) : Store × Nat :=
  ({ stmts := (st.next, t) :: st.stmts, next := st.next + 1 }, st.next)

def insert (st : Store) (t : Triple) : Store × Nat :=
  if multi t.p = false then
    match findTriple st t with
    | some id => (st, id)
    | none => fresh st t
  else fresh st t

theorem findTriple_none {st : Store} {t : Triple} (h : findTriple st t = none) :
    ∀ e ∈ st.stmts, e.2 ≠ t := by
  intro e he heq
  unfold findTriple at h
  rw [Option.map_eq_none_iff, List.find?_eq_none] at h
  exact h e he (by simp [heq])

theorem findTriple_some {st : Store} {t : Triple} {id : Nat} (h : findTriple st t = some id) :
    (id, t) ∈ st.stmts := by
  unfold findTriple at h
  rw [Option.map_eq_some_iff] at h
  obtain ⟨e, he, rfl⟩ := h
  have hm := List.mem_of_find?_eq_some he
  have hp := List.find?_some he
  simp at hp
  rw [← hp]; exact hm

theorem fresh_wf {st : Store} {t : Triple} (hwf : WF multi st) (hpos : 0 < st.next)
    (hnew : multi t.p = false → ∀ e ∈ st.stmts, e.2 ≠ t) : WF multi (fresh st t).1 where
  pos := by
    intro e he
    simp [fresh] at he
    rcases he with rfl | he
    · exact hpos
    · exact hwf.pos _ he
  lt_next := by
    intro e he
    simp [fresh] at he
    rcases he with rfl | he
    · simp [fresh]
    · have := hwf.lt_next _ he; simp [fresh]; omega
  nodup := by
    simp only [fresh, ids, List.map_cons, List.nodup_cons]
    refine ⟨?_, hwf.nodup⟩
    intro hmem
    rw [List.mem_map] at hmem
    obtain ⟨e, he, heq⟩ := hmem
    have := hwf.lt_next e he
    omega
  unique := by
    intro e₁ h₁ e₂ h₂ heq hm
    simp [fresh] at h₁ h₂
    rcases h₁ with rfl | h₁ <;> rcases h₂ with rfl | h₂
    · rfl
    · exact absurd heq.symm (hnew hm _ h₂)
    · have heq' : e₁.2 = t := heq
      exact absurd heq' (hnew (by rw [← heq']; exact hm) _ h₁)
    · exact hwf.unique _ h₁ _ h₂ heq hm

/-- Insert preserves every invariant. -/
theorem insert_wf {st : Store} {t : Triple} (hwf : WF multi st) (hpos : 0 < st.next) :
    WF multi (insert multi st t).1 := by
  unfold insert
  split
  · rename_i hm
    split
    · exact hwf
    · rename_i hnone
      exact fresh_wf multi hwf hpos (fun _ => findTriple_none hnone)
  · rename_i hm
    exact fresh_wf multi hwf hpos (fun h => absurd h hm)

/-- The allocator never moves backwards (rule `id-never-reused`). -/
theorem insert_next_mono (st : Store) (t : Triple) : st.next ≤ (insert multi st t).1.next := by
  unfold insert fresh; split
  · split <;> simp
  · simp

/-- A newly allocated ID is above every ID that was ever allocated before. -/
theorem insert_id_fresh_or_existing {st : Store} {t : Triple} :
    let r := insert multi st t
    (r.1 = st ∧ (r.2, t) ∈ st.stmts) ∨ (r.2 = st.next ∧ r.1.next = st.next + 1) := by
  unfold insert
  split
  · split
    · rename_i id hsome; exact Or.inl ⟨rfl, findTriple_some hsome⟩
    · exact Or.inr ⟨rfl, rfl⟩
  · exact Or.inr ⟨rfl, rfl⟩

/-- Scenario "Repeated insert is idempotent": on a non-multi predicate a second
insert of the same triple returns the same ID and changes nothing. -/
theorem insert_idempotent {st : Store} {t : Triple} (hm : multi t.p = false) :
    let r := insert multi st t
    insert multi r.1 t = r := by
  simp only [insert, hm]
  cases hf : findTriple st t with
  | some id => simp [hf]
  | none => simp [fresh, findTriple]

/-- Scenario "Multi predicate keeps parallel statements". -/
theorem insert_multi_distinct {st : Store} {t : Triple} (hm : multi t.p = true) :
    let r := insert multi st t
    (insert multi r.1 t).2 ≠ r.2 := by
  simp [insert, hm, fresh]

/-! ## Update in place (rule `dm-update-in-place`) -/

def replaceObj (id : Nat) (t' : Triple) (e : Entry) : Entry :=
  if e.1 = id then (e.1, t') else e

def update (st : Store) (id : Nat) (o : Term) : Option Store :=
  match lookup st id with
  | none => none
  | some t =>
    let t' : Triple := { t with o := o }
    if multi t.p = false ∧ st.stmts.any (fun e => e.2 == t' && e.1 != id) then none
    else some { st with stmts := st.stmts.map (replaceObj id t') }

theorem lookup_some {st : Store} {id : Nat} {t : Triple} (h : lookup st id = some t) :
    (id, t) ∈ st.stmts := by
  unfold lookup at h
  rw [Option.map_eq_some_iff] at h
  obtain ⟨e, he, rfl⟩ := h
  have hp := List.find?_some he
  simp at hp
  rw [← hp]; exact List.mem_of_find?_eq_some he

theorem replaceObj_fst (id : Nat) (t' : Triple) (e : Entry) : (replaceObj id t' e).1 = e.1 := by
  unfold replaceObj; split <;> rfl

/-- The set of IDs is unchanged by an update: no ID is created or lost. -/
theorem update_ids {st st' : Store} {id : Nat} {o : Term}
    (h : update multi st id o = some st') : ids st' = ids st := by
  unfold update at h
  split at h
  · cases h
  · dsimp only at h
    split at h
    · cases h
    · cases h
      simp only [ids, List.map_map]
      congr 1
      funext e
      exact replaceObj_fst id _ e

theorem update_next {st st' : Store} {id : Nat} {o : Term}
    (h : update multi st id o = some st') : st'.next = st.next := by
  unfold update at h
  split at h
  · cases h
  · dsimp only at h
    split at h
    · cases h
    · cases h; rfl

/-- Scenario "Overwrite keeps ID and annotations": after an update, the same ID
reads the new triple. Annotations are other entries whose subject is
`.stmt id`; they are untouched because `replaceObj` only rewrites entry `id`. -/
theorem update_lookup {st st' : Store} {id : Nat} {o : Term} {t : Triple}
    (hl : lookup st id = some t)
    (h : update multi st id o = some st') : lookup st' id = some { t with o := o } := by
  unfold update at h
  rw [hl] at h
  simp only at h
  split at h
  · cases h
  · cases h
    unfold lookup at hl ⊢
    simp only [List.find?_map]
    have hcomp : ((fun e : Entry => e.1 == id) ∘ replaceObj id { t with o := o }) =
        (fun e : Entry => e.1 == id) := by
      funext e; simp [Function.comp, replaceObj_fst]
    rw [hcomp]
    rw [Option.map_eq_some_iff] at hl
    obtain ⟨e, he, rfl⟩ := hl
    have hp := List.find?_some he
    simp at hp
    simp [he, replaceObj, hp]

/-- Annotations survive an update: every entry other than `id` is unchanged. -/
theorem update_preserves_others {st st' : Store} {id : Nat} {o : Term}
    (h : update multi st id o = some st') :
    ∀ e ∈ st.stmts, e.1 ≠ id → e ∈ st'.stmts := by
  intro e he hne
  unfold update at h
  split at h
  · cases h
  · dsimp only at h
    split at h
    · cases h
    · cases h
      simp only [List.mem_map]
      exact ⟨e, he, by simp [replaceObj, hne]⟩

theorem update_wf {st st' : Store} {id : Nat} {o : Term}
    (hwf : WF multi st) (h : update multi st id o = some st') : WF multi st' := by
  have hids := update_ids multi h
  have hnext := update_next multi h
  unfold update at h
  split at h
  · cases h
  · rename_i t hl
    have _hmem := lookup_some hl
    dsimp only at h
    split at h
    · cases h
    · rename_i hguard
      cases h
      refine ⟨?_, ?_, ?_, ?_⟩
      · intro e he
        simp only [List.mem_map] at he
        obtain ⟨e₀, he₀, rfl⟩ := he
        rw [replaceObj_fst]; exact hwf.pos _ he₀
      · intro e he
        simp only [List.mem_map] at he
        obtain ⟨e₀, he₀, rfl⟩ := he
        rw [replaceObj_fst]; exact hwf.lt_next _ he₀
      · rw [hids]; exact hwf.nodup
      · intro e₁ h₁ e₂ h₂ heq hm
        simp only [List.mem_map] at h₁ h₂
        obtain ⟨a, ha, rfl⟩ := h₁
        obtain ⟨b, hb, rfl⟩ := h₂
        simp only [replaceObj] at heq hm ⊢
        by_cases hai : a.1 = id <;> by_cases hbi : b.1 = id <;> simp only [hai, hbi, ite_true, ite_false] at heq hm ⊢
        · -- `a` was rewritten, `b` was not: the guard forbids `b.2 = t'`
          exfalso
          have hp : multi t.p = false := by simpa using hm
          apply hguard
          refine ⟨hp, ?_⟩
          rw [List.any_eq_true]
          exact ⟨b, hb, by simp [heq.symm, hbi]⟩
        · exfalso
          have hp : multi t.p = false := by
            have : (a.2).p = t.p := by rw [heq]
            rw [← this]; exact hm
          apply hguard
          refine ⟨hp, ?_⟩
          rw [List.any_eq_true]
          exact ⟨a, ha, by simp [heq, hai]⟩
        · exact hwf.unique _ ha _ hb heq hm

/-! ## Cascade delete (rule `dm-delete-cascade`, ADR 0006, ADR 0036)

Deleting `id` computes a doomed set `D`: `id`, every statement annotating a
member (subject position, always), and, when the caller passes `detach`, every
statement naming a member in object position. Both pulls are transitive. Without
`detach`, a delete is blocked (returns `none`, nothing changes) when a statement
outside `D` names a member of `D` in object position (ADR 0036). -/

/-- The term names a statement in `D`. -/
def termIn (D : List Nat) : Term → Bool
  | .stmt i => D.contains i
  | _ => false

/-- The entry's subject is a statement in `D`: it annotates something doomed. -/
def refersTo (D : List Nat) (e : Entry) : Bool := termIn D e.2.s

/-- The entry's object is a statement in `D`. -/
def objRefersTo (D : List Nat) (e : Entry) : Bool := termIn D e.2.o

/-- What the closure pulls into `D`: annotations always, object references only
under `detach`. -/
def pulls (detach : Bool) (D : List Nat) (e : Entry) : Bool :=
  refersTo D e || (detach && objRefersTo D e)

/-- No statement names, in subject or object position, a statement that does
not exist. -/
def RefClosed (st : Store) : Prop :=
  ∀ e ∈ st.stmts, ∀ i, (e.2.s = .stmt i ∨ e.2.o = .stmt i) → i ∈ ids st

/-- `D` is closed under what the delete pulls in. -/
def Closed (st : Store) (detach : Bool) (D : List Nat) : Prop :=
  ∀ e ∈ st.stmts, pulls detach D e = true → e.1 ∈ D

def closedB (st : Store) (detach : Bool) (D : List Nat) : Bool :=
  st.stmts.all (fun e => !pulls detach D e || D.contains e.1)

/-- The object guard of ADR 0036: some statement outside `D` names a member of
`D` in object position. -/
def blockedB (st : Store) (D : List Nat) : Bool :=
  st.stmts.any (fun e => objRefersTo D e && !D.contains e.1)

theorem closedB_sound {st : Store} {detach : Bool} {D : List Nat}
    (h : closedB st detach D = true) : Closed st detach D := by
  intro e he hr
  unfold closedB at h
  rw [List.all_eq_true] at h
  have := h e he
  simp [hr] at this
  exact this

theorem closedB_complete {st : Store} {detach : Bool} {D : List Nat}
    (h : Closed st detach D) : closedB st detach D = true := by
  unfold closedB
  rw [List.all_eq_true]
  intro e he
  cases hp : pulls detach D e
  · rfl
  · simp [h e he hp]

theorem termIn_stmt {D : List Nat} {t : Term} {i : Nat} (ht : t = .stmt i) (hi : i ∈ D) :
    termIn D t = true := by
  subst ht; simp [termIn, hi]

def removeAll (st : Store) (D : List Nat) : Store :=
  { st with stmts := st.stmts.filter (fun e => !D.contains e.1) }

/-- The key lemma: removing a set closed under annotation, when no survivor names
a member in object position, never leaves a dangling reference. -/
theorem removeAll_refClosed {st : Store} {detach : Bool} {D : List Nat}
    (hrc : RefClosed st) (hc : Closed st detach D) (hb : blockedB st D = false) :
    RefClosed (removeAll st D) := by
  intro e he i hs
  simp only [removeAll, List.mem_filter] at he
  obtain ⟨he, hnot⟩ := he
  have hi := hrc e he i hs
  have heD : e.1 ∉ D := by simpa using hnot
  have hiD : i ∉ D := by
    intro hiD
    rcases hs with hs | hs
    · have : pulls detach D e = true := by
        simp [pulls, refersTo, termIn_stmt hs hiD]
      exact heD (hc e he this)
    · have : blockedB st D = true := by
        unfold blockedB
        rw [List.any_eq_true]
        exact ⟨e, he, by simp [objRefersTo, termIn_stmt hs hiD, heD]⟩
      rw [hb] at this; cases this
  simp only [ids, List.mem_map] at hi ⊢
  obtain ⟨e', he', rfl⟩ := hi
  exact ⟨e', by simp [removeAll, List.mem_filter, he', hiD], rfl⟩

theorem removeAll_gone {st : Store} {D : List Nat} {i : Nat} (hi : i ∈ D) :
    i ∉ ids (removeAll st D) := by
  simp [ids, removeAll, List.mem_filter, hi]

theorem removeAll_wf {st : Store} {D : List Nat} (hwf : WF multi st) : WF multi (removeAll st D) where
  pos := fun e he => hwf.pos e (List.mem_filter.mp he).1
  lt_next := fun e he => hwf.lt_next e (List.mem_filter.mp he).1
  nodup := by
    simp only [ids, removeAll]
    exact (List.filter_sublist).map Prod.fst |>.nodup hwf.nodup
  unique := fun e₁ h₁ e₂ h₂ => hwf.unique e₁ (List.mem_filter.mp h₁).1 e₂ (List.mem_filter.mp h₂).1

/-- The statements one round adds: pulled in, not yet members. -/
def frontier (st : Store) (detach : Bool) (D : List Nat) : List Entry :=
  st.stmts.filter (fun e => pulls detach D e && !D.contains e.1)

/-- One round: add every statement the delete pulls in from a member of `D`. -/
def step (st : Store) (detach : Bool) (D : List Nat) : List Nat :=
  D ++ (frontier st detach D).map Prod.fst

def closure (st : Store) (detach : Bool) : Nat → List Nat → List Nat
  | 0, D => D
  | n + 1, D =>
    if (step st detach D).length = D.length then D else closure st detach n (step st detach D)

/-- The doomed set of deleting `id`, computed with fuel `|stmts| + 1`, which is
always enough (`closure_closed`, PO-CASCADE-FUEL). -/
def doomed (st : Store) (id : Nat) (detach : Bool) : List Nat :=
  closure st detach (st.stmts.length + 1) [id]

/-- Cascade delete (ADR 0006, ADR 0036). Fails, changing nothing, when the
object guard blocks it. The closedness check never fails (`closure_closed`); it
is kept so the model is safe by construction. -/
def cascadeDelete (st : Store) (id : Nat) (detach : Bool) : Option Store :=
  let D := doomed st id detach
  if closedB st detach D && !blockedB st D then some (removeAll st D) else none

theorem closure_contains (st : Store) (detach : Bool) : ∀ (n : Nat) (D : List Nat) (i : Nat),
    i ∈ D → i ∈ closure st detach n D
  | 0, _, _, h => h
  | n + 1, D, i, h => by
    unfold closure
    split
    · exact h
    · exact closure_contains st detach n _ i (List.mem_append_left _ h)

theorem id_mem_doomed (st : Store) (id : Nat) (detach : Bool) : id ∈ doomed st id detach :=
  closure_contains st detach _ _ id (List.mem_singleton_self id)

/-! ### Fuel adequacy (PO-CASCADE-FUEL)

The measure is the number of entries whose ID is not yet in `D`. A round that
adds nothing is a fixpoint, hence closed; any other round strictly lowers the
measure, which starts at most `|stmts|`. -/

/-- Entries of `st` whose ID is not in `D`. -/
def outside (st : Store) (D : List Nat) : Nat :=
  st.stmts.countP (fun e => !D.contains e.1)

theorem countP_lt_of_imp {α : Type} {p q : α → Bool} :
    ∀ {l : List α}, (∀ x ∈ l, p x = true → q x = true) →
    (∃ x ∈ l, q x = true ∧ p x = false) → l.countP p < l.countP q
  | [], _, ⟨_, hx, _⟩ => absurd hx List.not_mem_nil
  | a :: l, himp, ⟨x, hx, hqx, hpx⟩ => by
    simp only [List.countP_cons]
    have himp' : ∀ y ∈ l, p y = true → q y = true :=
      fun y hy => himp y (List.mem_cons_of_mem a hy)
    have hle : l.countP p ≤ l.countP q :=
      List.countP_mono_left (fun y hy => himp' y hy)
    rcases List.mem_cons.mp hx with rfl | hx
    · simp [hqx, hpx]; omega
    · have := countP_lt_of_imp himp' ⟨x, hx, hqx, hpx⟩
      have ha := himp a List.mem_cons_self
      cases hpa : p a <;> cases hqa : q a <;> simp_all <;> omega

theorem step_fixpoint_closed {st : Store} {detach : Bool} {D : List Nat}
    (h : (step st detach D).length = D.length) : Closed st detach D := by
  intro e he hp
  simp only [step, List.length_append, List.length_map] at h
  have hnil : frontier st detach D = [] := List.eq_nil_of_length_eq_zero (by omega)
  unfold frontier at hnil
  rw [List.filter_eq_nil_iff] at hnil
  have := hnil e he
  simp [hp] at this
  exact this

theorem outside_step_lt {st : Store} {detach : Bool} {D : List Nat}
    (h : (step st detach D).length ≠ D.length) :
    outside st (step st detach D) < outside st D := by
  unfold outside
  apply countP_lt_of_imp
  · intro x _ hx
    simp [step] at hx ⊢
    exact hx.1
  · have hne : frontier st detach D ≠ [] := by
      intro hnil
      simp [step, hnil] at h
    obtain ⟨e, he⟩ := List.exists_mem_of_ne_nil _ hne
    have he' := List.mem_filter.mp he
    refine ⟨e, he'.1, ?_, ?_⟩
    · have := he'.2
      simp only [Bool.and_eq_true] at this
      exact this.2
    · simp only [step, Bool.not_eq_false']
      simp only [List.contains_iff_mem, List.mem_append, List.mem_map]
      exact Or.inr ⟨e, he, rfl⟩

theorem closure_closed_of_lt (st : Store) (detach : Bool) : ∀ (n : Nat) (D : List Nat),
    outside st D < n → Closed st detach (closure st detach n D)
  | 0, _, h => absurd h (Nat.not_lt_zero _)
  | n + 1, D, h => by
    unfold closure
    split
    · rename_i hfix; exact step_fixpoint_closed hfix
    · rename_i hne
      exact closure_closed_of_lt st detach n _ (by have := outside_step_lt hne; omega)

/-- PO-CASCADE-FUEL: fuel `|stmts| + 1` always reaches a closed set, so
`cascadeDelete` never fails for lack of fuel, only on the object guard. -/
theorem closure_closed (st : Store) (id : Nat) (detach : Bool) :
    Closed st detach (doomed st id detach) :=
  closure_closed_of_lt st detach _ _
    (Nat.lt_succ_of_le List.countP_le_length)

/-- Scenario "Annotation chain is removed": after a cascade delete the deleted
statement is gone, no reference (subject or object) dangles, and all invariants
still hold. -/
theorem cascadeDelete_correct {st st' : Store} {id : Nat} {detach : Bool}
    (hwf : WF multi st) (hrc : RefClosed st) (h : cascadeDelete st id detach = some st') :
    WF multi st' ∧ RefClosed st' ∧ id ∉ ids st' := by
  unfold cascadeDelete at h
  simp only at h
  split at h
  · rename_i hc
    simp only [Bool.and_eq_true, Bool.not_eq_true'] at hc
    cases h
    exact ⟨removeAll_wf multi hwf,
      removeAll_refClosed hrc (closedB_sound hc.1) hc.2,
      removeAll_gone (id_mem_doomed st id detach)⟩
  · cases h

/-- Exactly the doomed set is removed: every statement not in it survives. -/
theorem cascadeDelete_removed {st st' : Store} {id : Nat} {detach : Bool}
    (h : cascadeDelete st id detach = some st') :
    st' = removeAll st (doomed st id detach) := by
  unfold cascadeDelete at h
  simp only at h
  split at h
  · cases h; rfl
  · cases h

/-- Deleting never moves the allocator: together with `insert_next_mono` and
`update_next`, every operation keeps `next` monotone, so a deleted ID is never
handed out again (scenario "Deleted IDs are not reused"). -/
theorem cascadeDelete_next {st st' : Store} {id : Nat} {detach : Bool}
    (h : cascadeDelete st id detach = some st') : st'.next = st.next := by
  rw [cascadeDelete_removed h]; rfl

/-- A delete fails exactly when the object guard fires (never for fuel). -/
theorem cascadeDelete_none_iff {st : Store} {id : Nat} {detach : Bool} :
    cascadeDelete st id detach = none ↔ blockedB st (doomed st id detach) = true := by
  unfold cascadeDelete
  simp only [closedB_complete (closure_closed st id detach), Bool.true_and]
  cases blockedB st (doomed st id detach) <;> simp

/-- `DETACH` is never blocked: object references are pulled into the doomed set. -/
theorem cascadeDelete_detach_some (st : Store) (id : Nat) :
    cascadeDelete st id true ≠ none := by
  rw [Ne, cascadeDelete_none_iff]
  have hc := closure_closed st id true
  unfold blockedB
  simp only [List.any_eq_true, Bool.and_eq_true, Bool.not_eq_true', not_exists, not_and]
  intro e he ho hn
  have : e.1 ∈ doomed st id true := hc e he (by simp [pulls, ho])
  simp [this] at hn

/-- Guard necessity (ADR 0036): a blocked delete is a plain delete, and some
statement outside the doomed set names a doomed statement in object position, so
removing the doomed set really would leave a dangling object. -/
theorem cascadeDelete_blocked {st : Store} {id : Nat} {detach : Bool}
    (h : cascadeDelete st id detach = none) :
    detach = false ∧
    (∃ e ∈ st.stmts, ∃ i, e.2.o = .stmt i ∧ i ∈ doomed st id detach ∧
      e.1 ∉ doomed st id detach) ∧
    ¬ RefClosed (removeAll st (doomed st id detach)) := by
  have hd : detach = false := by
    cases detach
    · rfl
    · exact absurd h (cascadeDelete_detach_some st id)
  have hb := cascadeDelete_none_iff.mp h
  unfold blockedB at hb
  rw [List.any_eq_true] at hb
  obtain ⟨e, he, hx⟩ := hb
  simp only [Bool.and_eq_true, Bool.not_eq_true', objRefersTo] at hx
  obtain ⟨ho, hn⟩ := hx
  have hn' : e.1 ∉ doomed st id detach := by simpa using hn
  cases hoe : e.2.o with
  | node _ => simp [termIn, hoe] at ho
  | lit _ => simp [termIn, hoe] at ho
  | stmt i =>
    have hi : i ∈ doomed st id detach := by simpa [termIn, hoe] using ho
    refine ⟨hd, ⟨e, he, i, hoe, hi, hn'⟩, ?_⟩
    intro hrc
    have hmem : e ∈ (removeAll st (doomed st id detach)).stmts := by
      simp [removeAll, List.mem_filter, he, hn']
    exact removeAll_gone hi (hrc e hmem i (Or.inr hoe))

end

end MetaCore.Store
