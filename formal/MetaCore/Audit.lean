/-
  Axiom audit. `lake build MetaCore.Audit` prints the axioms each headline
  theorem depends on; CI (scripts/lean-audit.sh) fails if `sorryAx` appears.
  Only `propext`, `Classical.choice` and `Quot.sound` are acceptable.
-/
import MetaCore.Id
import MetaCore.Store
import MetaCore.Nesting
import MetaCore.Durability
import MetaCore.Hyperedge
import MetaCore.Graphs
import MetaCore.Datalog

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
#print axioms MetaCore.Id.packN_injective
#print axioms MetaCore.Id.packN_lt_iff
#print axioms MetaCore.Id.pack_payload_lt
#print axioms MetaCore.Id.encStr_injective
#print axioms MetaCore.Id.encStr_lt_iff
#print axioms MetaCore.Id.encStr_in_run
#print axioms MetaCore.Id.encLangStr_lt_iff
#print axioms MetaCore.Id.langPayload_lt
#print axioms MetaCore.Id.PosDbl.mag_lt_D
#print axioms MetaCore.Id.PosDbl.mag_strictMono
#print axioms MetaCore.Id.PosDbl.mag_lt_iff
#print axioms MetaCore.Id.encDblV_strictMono
#print axioms MetaCore.Id.encDblV_literal
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
#print axioms MetaCore.Store.removeAll_refClosed
#print axioms MetaCore.Store.closure_closed
#print axioms MetaCore.Store.cascadeDelete_removed
#print axioms MetaCore.Store.cascadeDelete_none_iff
#print axioms MetaCore.Store.cascadeDelete_detach_some
#print axioms MetaCore.Store.cascadeDelete_blocked
#print axioms MetaCore.Nesting.addEdge_acyclic
#print axioms MetaCore.Nesting.guard_necessary
#print axioms MetaCore.Nesting.scope_trans
#print axioms MetaCore.Durability.durable_survives
#print axioms MetaCore.Durability.relaxed_covered
#print axioms MetaCore.Durability.crash_outcome_exists
#print axioms MetaCore.Hyperedge.shapeB_iff
#print axioms MetaCore.Hyperedge.satisfies_arity
#print axioms MetaCore.Hyperedge.satisfies_fixed_arity
#print axioms MetaCore.Hyperedge.create_some
#print axioms MetaCore.Hyperedge.create_none_iff
#print axioms MetaCore.Hyperedge.create_hwf
#print axioms MetaCore.Hyperedge.create_hrefClosed
#print axioms MetaCore.Hyperedge.empty_hwf
#print axioms MetaCore.Hyperedge.insert_hwf
#print axioms MetaCore.Hyperedge.insert_hrefClosed
#print axioms MetaCore.Hyperedge.update_hwf
#print axioms MetaCore.Hyperedge.update_hrefClosed
#print axioms MetaCore.Hyperedge.store_insert_refClosed
#print axioms MetaCore.Hyperedge.store_update_refClosed
#print axioms MetaCore.Hyperedge.cascadeDelete_correct
#print axioms MetaCore.Hyperedge.cascadeDelete_removed
#print axioms MetaCore.Hyperedge.cascadeDelete_plain_keeps_hedges
#print axioms MetaCore.Hyperedge.cascadeDelete_hedges_sub
#print axioms MetaCore.Hyperedge.cascadeDelete_hedge
#print axioms MetaCore.Hyperedge.cascadeDelete_next
#print axioms MetaCore.Hyperedge.cascadeDelete_none_iff
#print axioms MetaCore.Hyperedge.cascadeDelete_detach_some
#print axioms MetaCore.Hyperedge.cascadeDelete_hedge_blocked
#print axioms MetaCore.Graphs.reachB_iff
#print axioms MetaCore.Graphs.apply_memOK
#print axioms MetaCore.Graphs.apply_acyclic
#print axioms MetaCore.Graphs.apply_nest
#print axioms MetaCore.Graphs.scope_mem
#print axioms MetaCore.Graphs.scope_nodup
#print axioms MetaCore.Graphs.scope_sorted
#print axioms MetaCore.Graphs.apply_insert_member
#print axioms MetaCore.Graphs.apply_removeMember
#print axioms MetaCore.Graphs.apply_hwf
#print axioms MetaCore.Graphs.apply_hrefClosed
#print axioms MetaCore.Graphs.apply_next_mono
#print axioms MetaCore.Graphs.empty_inv
#print axioms MetaCore.Graphs.apply_inv
#print axioms MetaCore.Datalog.safeB_iff
#print axioms MetaCore.Datalog.programSafeB_iff
#print axioms MetaCore.Datalog.stratB_iff_not_negCycle
#print axioms MetaCore.Datalog.negCycle_rejected
#print axioms MetaCore.Datalog.negReachB_iff
#print axioms MetaCore.Datalog.stratum_valid
#print axioms MetaCore.Datalog.IsStrat.reach_le
#print axioms MetaCore.Datalog.stratB_complete
#print axioms MetaCore.Datalog.stratB_iff
#print axioms MetaCore.Datalog.not_negCycle_iff_strat
#print axioms MetaCore.Datalog.IsStrat.neg_lower
#print axioms MetaCore.Datalog.IsStrat.pos_le
#print axioms MetaCore.Datalog.naive_closed
#print axioms MetaCore.Datalog.naive_base
#print axioms MetaCore.Datalog.mem_naive_iff
#print axioms MetaCore.Datalog.naive_least
#print axioms MetaCore.Datalog.snInv_init
#print axioms MetaCore.Datalog.snInv_step
#print axioms MetaCore.Datalog.semiNaive_closed
#print axioms MetaCore.Datalog.semiNaive_eq_naive
#print axioms MetaCore.Datalog.evalSemiNaive_eq_evalNaive
#print axioms MetaCore.Datalog.assigns_complete
#print axioms MetaCore.Datalog.mem_ground
#print axioms MetaCore.Datalog.PDerivable.args_adom
#print axioms MetaCore.Datalog.mem_evalNaive_iff
#print axioms MetaCore.Datalog.mem_evalSemiNaive_iff
#print axioms MetaCore.Datalog.GL.agree
#print axioms MetaCore.Datalog.stable_unique
#print axioms MetaCore.Datalog.le_top
#print axioms MetaCore.Datalog.stage_new
#print axioms MetaCore.Datalog.stage_low
#print axioms MetaCore.Datalog.stage_sound
#print axioms MetaCore.Datalog.stage_complete
#print axioms MetaCore.Datalog.evalAlong_stable
#print axioms MetaCore.Datalog.evalAlong_indep
#print axioms MetaCore.Datalog.evalProgram_correct
#print axioms MetaCore.Datalog.evalProgram_none_iff
