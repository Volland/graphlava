/-
  Axiom audit. `lake build MetaCore.Audit` prints the axioms each headline
  theorem depends on; CI (scripts/lean-audit.sh) fails if `sorryAx` appears.
  Only `propext`, `Classical.choice` and `Quot.sound` are acceptable.
-/
import MetaCore.Id
import MetaCore.Store
import MetaCore.Nesting
import MetaCore.Durability

#print axioms MetaCore.Id.kind_mk
#print axioms MetaCore.Id.mk_injective
#print axioms MetaCore.Id.mk_lt_of_kind_lt
#print axioms MetaCore.Id.chunkOf_chunked
#print axioms MetaCore.Id.decInt_encInt
#print axioms MetaCore.Id.encInt_lt_iff
#print axioms MetaCore.Id.encInt_literal
#print axioms MetaCore.Id.nonliteral_lt_encInt
#print axioms MetaCore.Id.encDbl_strictMono
#print axioms MetaCore.Id.negZero_lt_posZero
#print axioms MetaCore.Id.double_before_int
#print axioms MetaCore.Id.int_before_decimal
#print axioms MetaCore.Store.insert_wf
#print axioms MetaCore.Store.insert_next_mono
#print axioms MetaCore.Store.insert_idempotent
#print axioms MetaCore.Store.insert_multi_distinct
#print axioms MetaCore.Store.update_ids
#print axioms MetaCore.Store.update_lookup
#print axioms MetaCore.Store.update_preserves_others
#print axioms MetaCore.Store.update_wf
#print axioms MetaCore.Store.cascadeDelete_correct
#print axioms MetaCore.Store.cascadeDelete_next
#print axioms MetaCore.Nesting.addEdge_acyclic
#print axioms MetaCore.Nesting.guard_necessary
#print axioms MetaCore.Nesting.scope_trans
#print axioms MetaCore.Durability.durable_survives
#print axioms MetaCore.Durability.relaxed_covered
#print axioms MetaCore.Durability.crash_outcome_exists
