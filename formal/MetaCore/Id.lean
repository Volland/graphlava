/-
  # ID encoding (tg.md/02-id-encoding.md, ADR 0011)

  The 64-bit ID is modelled as a natural number below 2^64. Bit fields are
  modelled arithmetically (`kind * 2^60 + payload`) rather than with bitwise
  operators: the two are equal for in-range fields, and arithmetic lets `omega`
  discharge the obligations. The Rust implementation must agree with these
  definitions; the oracle traces check that it does (refinement bridge).
-/
namespace MetaCore.Id

/-- 2^60: the weight of the 4-bit kind tag (bits 63..60) of non-literal IDs. -/
abbrev K : Nat := 1152921504606846976
/-- 2^62: the weight of the 2-bit inline-literal class (bits 63..62): `10` double, `11` other. -/
abbrev Q : Nat := 4611686018427387904
/-- 2^58: the weight of the inline-literal subtype in class `11` (bits 61..58). -/
abbrev S : Nat := 288230376151711744
/-- 2^57: the offset-binary bias for signed 58-bit payloads. -/
abbrev B : Nat := 144115188075855872
/-- 2^61: the bias of the inline-double payload and the bound on its magnitude field. -/
abbrev D : Nat := 2305843009213693952
/-- 2^20: IDs per chunk. -/
abbrev C : Nat := 1048576
/-- 2^40: number of chunks per chunked kind. -/
abbrev NC : Nat := 1099511627776
/-- 2^64. -/
abbrev U64 : Nat := 18446744073709551616

/-- `id_omega` after unfolding the width constants to numerals. -/
macro "id_omega" : tactic =>
  `(tactic| ((try simp only [K, Q, S, B, D, C, NC, U64] at *) <;> omega))

/-! ## Kind tag -/

def mk (kind payload : Nat) : Nat := kind * K + payload
def kind (id : Nat) : Nat := id / K
def payload (id : Nat) : Nat := id % K

/-- Rule `id-kind-tag`: the kind is readable from the ID alone. -/
theorem kind_mk {k p : Nat} (hp : p < K) : kind (mk k p) = k := by
  unfold kind mk; id_omega

theorem payload_mk {k p : Nat} (hp : p < K) : payload (mk k p) = p := by
  unfold payload mk; id_omega

/-- An in-range kind and payload always fit in a `u64`. -/
theorem mk_lt_u64 {k p : Nat} (hk : k < 16) (hp : p < K) : mk k p < U64 := by
  unfold mk; id_omega

/-- Encoding is injective: distinct (kind, payload) pairs give distinct IDs. -/
theorem mk_injective {k₁ p₁ k₂ p₂ : Nat} (h₁ : p₁ < K) (h₂ : p₂ < K)
    (h : mk k₁ p₁ = mk k₂ p₂) : k₁ = k₂ ∧ p₁ = p₂ := by
  unfold mk at h; id_omega

/-- IDs sort by kind first. Inline literals have bit 63 set (tags `0x8`..`0xF`),
so every inline literal sorts after every non-literal (rule `id-kind-tag`). -/
theorem mk_lt_of_kind_lt {k₁ p₁ k₂ p₂ : Nat} (hp : p₁ < K) (hk : k₁ < k₂) :
    mk k₁ p₁ < mk k₂ p₂ := by
  unfold mk
  have : (k₁ + 1) * K ≤ k₂ * K := Nat.mul_le_mul_right K hk
  rw [Nat.add_mul, Nat.one_mul] at this
  id_omega

/-- The null ID (kind 0, payload 0) is below every allocated ID. -/
theorem null_least {k p : Nat} (hk : 0 < k) : mk 0 0 < mk k p := by
  unfold mk; have : K ≤ k * K := Nat.le_mul_of_pos_left K hk; id_omega

/-! ## Chunked kinds (rule `id-chunk-split`) -/

def chunked (kind chunk offset : Nat) : Nat := mk kind (chunk * C + offset)
def chunkOf (id : Nat) : Nat := payload id / C
def offsetOf (id : Nat) : Nat := payload id % C

theorem chunk_payload_lt {c o : Nat} (hc : c < NC) (ho : o < C) : c * C + o < K := by
  have : c * C + o < (c + 1) * C := by rw [Nat.add_mul, Nat.one_mul]; id_omega
  have : (c + 1) * C ≤ NC * C := Nat.mul_le_mul_right C hc
  id_omega

/-- The chunk number, and therefore the home graph, is a pure function of the ID. -/
theorem chunkOf_chunked {k c o : Nat} (hc : c < NC) (ho : o < C) :
    chunkOf (chunked k c o) = c ∧ offsetOf (chunked k c o) = o ∧ kind (chunked k c o) = k := by
  have hp := chunk_payload_lt hc ho
  unfold chunkOf offsetOf chunked
  rw [payload_mk hp, kind_mk hp]
  refine ⟨?_, ?_, rfl⟩ <;> id_omega

/-- The system graph has the fixed ID `0x5000_0000_0000_0000` (rule `id-graph-bootstrap`). -/
theorem system_graph_id : chunked 5 0 0 = 0x5000000000000000 := by decide

/-! ## Inline literals (rule `id-inline-layout`)

Bit 63 set marks an inline literal. Class `10` (bits 63..62) is an inline double
with a 62-bit payload; class `11` holds every other inline subtype, with a 4-bit
subtype in bits 61..58 and a 58-bit payload. Subtype `0x0` is `xsd:integer` and
`0x1` is `xsd:decimal`, so doubles, integers and decimals are three adjacent runs. -/

/-- An ID in class `11` with subtype `sub` and a 58-bit payload. -/
def lit (sub payload : Nat) : Nat := 3 * Q + sub * S + payload

/-! ### Inline integers (rule `id-inline-range`, offset-binary transform) -/

def intMin : Int := -(B : Int)
def intMax : Int := (B : Int) - 1

def InIntRange (x : Int) : Prop := intMin ≤ x ∧ x ≤ intMax

/-- Payload of an inline integer: offset-binary, `x + 2^57`. -/
def intPayload (x : Int) : Nat := (x + (B : Int)).toNat

/-- Full ID of an inline integer: class `11`, subtype `0x0`. -/
def encInt (x : Int) : Nat := lit 0 (intPayload x)

def decInt (id : Nat) : Int := ((id % S : Nat) : Int) - (B : Int)

theorem intPayload_lt {x : Int} (h : InIntRange x) : intPayload x < S := by
  unfold InIntRange intMin intMax at h; unfold intPayload; id_omega

/-- Round trip: `decode (encode x) = x` for every in-range integer. -/
theorem decInt_encInt {x : Int} (h : InIntRange x) : decInt (encInt x) = x := by
  unfold InIntRange intMin intMax at h
  unfold decInt encInt lit intPayload
  id_omega

/-- Order preservation: unsigned ID order equals integer order. This is what
lets `?v > c` compile to a key range within the integer run. -/
theorem encInt_strictMono {a b : Int} (ha : InIntRange a) (hb : InIntRange b) (h : a < b) :
    encInt a < encInt b := by
  unfold InIntRange intMin intMax at ha hb
  unfold encInt lit intPayload
  id_omega

theorem encInt_lt_iff {a b : Int} (ha : InIntRange a) (hb : InIntRange b) :
    encInt a < encInt b ↔ a < b := by
  constructor
  · intro h
    refine Int.lt_of_not_ge fun hge => ?_
    rcases Int.lt_or_eq_of_le hge with hlt | heq
    · exact Nat.lt_asymm h (encInt_strictMono hb ha hlt)
    · subst heq; exact Nat.lt_irrefl _ h
  · exact encInt_strictMono ha hb

/-- Every inline integer is an inline-literal ID (tag at least `0x8`). -/
theorem encInt_literal {x : Int} (h : InIntRange x) : 8 ≤ kind (encInt x) := by
  unfold InIntRange intMin intMax at h
  unfold kind encInt lit intPayload; id_omega

/-- Every inline integer sorts after every non-literal ID. -/
theorem nonliteral_lt_encInt {k p : Nat} {x : Int} (hk : k < 8) (hp : p < K)
    (h : InIntRange x) : mk k p < encInt x := by
  unfold InIntRange intMin intMax at h
  unfold mk encInt lit intPayload; id_omega

/-! ### Inline doubles (rule `id-double-inline`)

The magnitude field `m` is the 9-bit rebiased exponent followed by the 52
mantissa bits, so for non-negative doubles `m` order is value order (the IEEE
property this model assumes). Negative values are mirrored below the bias. -/

/-- Payload of an inline double from its sign and 61-bit magnitude field. -/
def dblPayload : Bool → Nat → Nat
  | true, m => D - 1 - m
  | false, m => D + m

/-- Full ID of an inline double: class `10`. -/
def encDbl (neg : Bool) (m : Nat) : Nat := 2 * Q + dblPayload neg m

/-- Value order on (sign, magnitude) pairs: every negative value is below every
non-negative one, `-0.0` included, and a larger magnitude is further from zero. -/
def DblLt : Bool × Nat → Bool × Nat → Prop
  | (true, m₁), (true, m₂) => m₂ < m₁
  | (true, _), (false, _) => True
  | (false, _), (true, _) => False
  | (false, m₁), (false, m₂) => m₁ < m₂

/-- The sign-flip transform is strictly monotone, and `-0.0 < +0.0` as distinct
IDs (PO-ID-DOUBLE, for the transform). -/
theorem encDbl_strictMono {n₁ n₂ : Bool} {m₁ m₂ : Nat} (h₁ : m₁ < D) (h₂ : m₂ < D)
    (h : DblLt (n₁, m₁) (n₂, m₂)) : encDbl n₁ m₁ < encDbl n₂ m₂ := by
  cases n₁ <;> cases n₂ <;> simp only [DblLt] at h <;>
    simp only [encDbl, dblPayload] <;> id_omega

theorem negZero_lt_posZero : encDbl true 0 < encDbl false 0 := by
  simp only [encDbl, dblPayload]; id_omega

/-- Every inline double is an inline-literal ID (tag at least `0x8`). -/
theorem encDbl_literal {n : Bool} {m : Nat} (hm : m < D) : 8 ≤ kind (encDbl n m) := by
  cases n <;> simp only [kind, encDbl, dblPayload] <;>
    id_omega

/-- Rule `id-numeric-adjacent`: every inline double sorts before every inline
integer, and every integer before every decimal (subtype `0x1`), so the three
numeric runs are adjacent. -/
theorem double_before_int {n : Bool} {m : Nat} {x : Int} (hm : m < D) (h : InIntRange x) :
    encDbl n m < encInt x := by
  unfold InIntRange intMin intMax at h
  cases n <;> simp only [encDbl, dblPayload, encInt, lit, intPayload] <;> id_omega

theorem int_before_decimal {x : Int} {d : Nat} (h : InIntRange x) (_hd : d < S) :
    encInt x < lit 1 d := by
  unfold InIntRange intMin intMax at h
  unfold encInt lit intPayload; id_omega

end MetaCore.Id
