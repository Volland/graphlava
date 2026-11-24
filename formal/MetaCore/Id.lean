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
/-- 2^52: the weight of the double exponent field above the 52-bit mantissa. -/
abbrev M52 : Nat := 4503599627370496
/-- 2^64. -/
abbrev U64 : Nat := 18446744073709551616

/-- `id_omega` after unfolding the width constants to numerals. -/
macro "id_omega" : tactic =>
  `(tactic| ((try simp only [K, Q, S, B, D, C, NC, M52, U64] at *) <;> omega))

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
mantissa bits; `PosDbl.mag_strictMono` (below) proves that `m` order is value
order for non-negative doubles. Negative values are mirrored below the bias. -/

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

/-! ### Inline strings (subtype `0x5`) and language-tagged strings (`0x6`)

A string of at most 7 UTF-8 bytes is packed big-endian into 56 bits, zero padded
on the right, and placed in payload bits 57..2 (`pack bs * 4`). Zero padding
makes `s` and `s ++ [0]` collide, so a string whose last byte is `0x00` is not
inline (PO-ID-STRING precondition). -/

/-- `packN n bs`: the first `n` bytes of `bs`, zero padded to `n` bytes, read as
a big-endian number. -/
def packN : Nat → List Nat → Nat
  | 0, _ => 0
  | _ + 1, [] => 0
  | n + 1, b :: bs => b * 256 ^ n + packN n bs

/-- Big-endian, zero-padded 7-byte packing of an inline string. -/
def pack (bs : List Nat) : Nat := packN 7 bs

/-- Bytes are below 256, at most `n` of them, and the last one is not `0x00`. -/
def Packable (n : Nat) (bs : List Nat) : Prop :=
  (∀ b ∈ bs, b < 256) ∧ bs.length ≤ n ∧ bs.getLast? ≠ some 0

/-- The inline-string precondition: at most 7 bytes, last byte nonzero. -/
abbrev InlineStr (bs : List Nat) : Prop := Packable 7 bs

/-- Full ID of an inline `xsd:string`: class `11`, subtype `0x5`. -/
def encStr (bs : List Nat) : Nat := lit 5 (pack bs * 4)

/-- Lexicographic order on byte lists; a proper prefix sorts first. -/
def LexLt : List Nat → List Nat → Prop
  | [], [] => False
  | [], _ :: _ => True
  | _ :: _, [] => False
  | a :: as, b :: bs => a < b ∨ (a = b ∧ LexLt as bs)

theorem Packable.tail {n b : Nat} {bs : List Nat} (h : Packable (n + 1) (b :: bs)) :
    Packable n bs := by
  obtain ⟨hb, hl, hz⟩ := h
  refine ⟨fun x hx => hb x (List.mem_cons_of_mem b hx), by simp at hl; omega, ?_⟩
  cases bs with
  | nil => simp
  | cons c cs =>
    rw [List.getLast?_cons] at hz
    simpa [List.getLast?_cons] using hz

theorem packN_lt : ∀ (n : Nat) (bs : List Nat), (∀ b ∈ bs, b < 256) → packN n bs < 256 ^ n
  | 0, _, _ => by simp [packN]
  | n + 1, [], _ => by simp [packN]; exact Nat.pos_of_ne_zero (by simp)
  | n + 1, b :: bs, h => by
    simp only [packN]
    have hb : b < 256 := h b List.mem_cons_self
    have ih := packN_lt n bs (fun x hx => h x (List.mem_cons_of_mem b hx))
    have : (b + 1) * 256 ^ n ≤ 256 * 256 ^ n := Nat.mul_le_mul_right _ hb
    rw [Nat.pow_succ, Nat.add_mul, Nat.one_mul] at *
    rw [Nat.mul_comm (256 ^ n) 256]
    omega

/-- A packable non-empty string packs to a positive number. -/
theorem packN_pos : ∀ (n : Nat) (bs : List Nat), Packable n bs → bs ≠ [] → 0 < packN n bs
  | 0, [], _, h => absurd rfl h
  | 0, _ :: _, ⟨_, hl, _⟩, _ => by simp at hl
  | n + 1, [], _, h => absurd rfl h
  | n + 1, b :: bs, hp, _ => by
    simp only [packN]
    cases bs with
    | nil =>
      have hb : b ≠ 0 := by have := hp.2.2; simpa using this
      have : 0 < 256 ^ n := Nat.pos_of_ne_zero (by simp)
      have : 256 ^ n ≤ b * 256 ^ n := Nat.le_mul_of_pos_left _ (Nat.pos_of_ne_zero hb)
      omega
    | cons c cs =>
      have := packN_pos n (c :: cs) hp.tail (by simp)
      omega

/-- Peeling the leading digit of a number in base `M`. -/
theorem digit_lt {x y a b M : Nat} (ha : a < M) (h : x < y) : x * M + a < y * M + b := by
  have : (x + 1) * M ≤ y * M := Nat.mul_le_mul_right M h
  rw [Nat.add_mul, Nat.one_mul] at this
  omega

theorem digit_eq {x y a b M : Nat} (ha : a < M) (hb : b < M) (h : x * M + a = y * M + b) :
    x = y ∧ a = b := by
  rcases Nat.lt_trichotomy x y with hxy | hxy | hxy
  · have := digit_lt (b := b) ha hxy; omega
  · subst hxy; omega
  · have := digit_lt (b := a) hb hxy; omega

/-- Packing is injective on packable strings. -/
theorem packN_injective : ∀ (n : Nat) {a b : List Nat}, Packable n a → Packable n b →
    packN n a = packN n b → a = b
  | 0, [], [], _, _, _ => rfl
  | 0, _ :: _, _, ⟨_, hl, _⟩, _, _ => by simp at hl
  | 0, _, _ :: _, _, ⟨_, hl, _⟩, _ => by simp at hl
  | n + 1, [], [], _, _, _ => rfl
  | n + 1, [], b :: bs, _, hb, h => by
    have := packN_pos (n + 1) (b :: bs) hb (by simp)
    have h0 : packN (n + 1) [] = 0 := rfl
    omega
  | n + 1, a :: as, [], ha, _, h => by
    have := packN_pos (n + 1) (a :: as) ha (by simp)
    have h0 : packN (n + 1) [] = 0 := rfl
    omega
  | n + 1, a :: as, b :: bs, ha, hb, h => by
    simp only [packN] at h
    obtain ⟨hab, hrest⟩ := digit_eq (packN_lt n as (fun x hx => ha.1 x (List.mem_cons_of_mem a hx)))
      (packN_lt n bs (fun x hx => hb.1 x (List.mem_cons_of_mem b hx))) h
    rw [hab, packN_injective n ha.tail hb.tail hrest]

/-- Packing is strictly monotone in lexicographic order. -/
theorem packN_strictMono : ∀ (n : Nat) {a b : List Nat}, Packable n a → Packable n b →
    LexLt a b → packN n a < packN n b
  | 0, [], [], _, _, h => absurd h (by simp [LexLt])
  | 0, _ :: _, _, ⟨_, hl, _⟩, _, _ => by simp at hl
  | 0, [], _ :: _, _, ⟨_, hl, _⟩, _ => by simp at hl
  | n + 1, [], [], _, _, h => absurd h (by simp [LexLt])
  | n + 1, [], b :: bs, _, hb, _ => by
    have := packN_pos (n + 1) (b :: bs) hb (by simp); simpa [packN] using this
  | n + 1, _ :: _, [], _, _, h => absurd h (by simp [LexLt])
  | n + 1, a :: as, b :: bs, ha, hb, h => by
    simp only [packN]
    have hla := packN_lt n as (fun x hx => ha.1 x (List.mem_cons_of_mem a hx))
    simp only [LexLt] at h
    rcases h with h | ⟨rfl, h⟩
    · exact digit_lt hla h
    · have := packN_strictMono n ha.tail hb.tail h; omega

theorem LexLt_trichotomy : ∀ (a b : List Nat), LexLt a b ∨ a = b ∨ LexLt b a
  | [], [] => Or.inr (Or.inl rfl)
  | [], _ :: _ => Or.inl trivial
  | _ :: _, [] => Or.inr (Or.inr trivial)
  | a :: as, b :: bs => by
    rcases Nat.lt_trichotomy a b with h | rfl | h
    · exact Or.inl (Or.inl h)
    · rcases LexLt_trichotomy as bs with h | rfl | h
      · exact Or.inl (Or.inr ⟨rfl, h⟩)
      · exact Or.inr (Or.inl rfl)
      · exact Or.inr (Or.inr (Or.inr ⟨rfl, h⟩))
    · exact Or.inr (Or.inr (Or.inl h))

theorem packN_lt_iff (n : Nat) {a b : List Nat} (ha : Packable n a) (hb : Packable n b) :
    packN n a < packN n b ↔ LexLt a b := by
  constructor
  · intro h
    rcases LexLt_trichotomy a b with h' | rfl | h'
    · exact h'
    · exact absurd h (Nat.lt_irrefl _)
    · exact absurd (packN_strictMono n hb ha h') (Nat.lt_asymm h)
  · exact packN_strictMono n ha hb

/-- The packed string fits payload bits 57..2. -/
theorem pack_payload_lt {bs : List Nat} (h : InlineStr bs) : pack bs * 4 < S := by
  have := packN_lt 7 bs h.1
  unfold pack; simp only [S]; simp at this; omega

/-- PO-ID-STRING, injectivity: distinct inline strings get distinct IDs. -/
theorem encStr_injective {a b : List Nat} (ha : InlineStr a) (hb : InlineStr b)
    (h : encStr a = encStr b) : a = b := by
  unfold encStr lit at h
  exact packN_injective 7 ha hb (by unfold pack at h; omega)

/-- PO-ID-STRING, order: unsigned ID order equals lexicographic byte order. -/
theorem encStr_lt_iff {a b : List Nat} (ha : InlineStr a) (hb : InlineStr b) :
    encStr a < encStr b ↔ LexLt a b := by
  rw [← packN_lt_iff 7 ha hb]
  unfold encStr lit pack
  omega

/-- Every inline string stays in its subtype run: above every integer, decimal,
boolean, date and dateTime ID and below subtype `0x6`. -/
theorem encStr_in_run {bs : List Nat} (h : InlineStr bs) :
    lit 5 0 ≤ encStr bs ∧ encStr bs < lit 6 0 := by
  have := pack_payload_lt h
  unfold encStr lit; id_omega

/-- Payload of an inline `rdf:langString`: 5 bytes big-endian, zero padded, then
a 16-bit language ID, in payload bits 57..2. -/
def langPayload (bs : List Nat) (lang : Nat) : Nat := (packN 5 bs * 65536 + lang) * 4

/-- Full ID of an inline `rdf:langString`: class `11`, subtype `0x6`. -/
def encLangStr (bs : List Nat) (lang : Nat) : Nat := lit 6 (langPayload bs lang)

/-- Language-tagged strings order lexicographically by text, then by language ID. -/
theorem encLangStr_lt_iff {a b : List Nat} {la lb : Nat} (ha : Packable 5 a) (hb : Packable 5 b)
    (hla : la < 65536) (hlb : lb < 65536) :
    encLangStr a la < encLangStr b lb ↔ LexLt a b ∨ (a = b ∧ la < lb) := by
  rw [← packN_lt_iff 5 ha hb]
  unfold encLangStr langPayload lit
  have hinj := fun h => packN_injective 5 ha hb h
  have hcong : a = b → packN 5 a = packN 5 b := fun h => h ▸ rfl
  generalize packN 5 a = pa at *
  generalize packN 5 b = pb at *
  constructor
  · intro h
    rcases Nat.lt_trichotomy pa pb with hp | hp | hp
    · exact Or.inl hp
    · subst hp; exact Or.inr ⟨hinj rfl, by omega⟩
    · have : pb * 65536 + lb < pa * 65536 + la := digit_lt hlb hp
      omega
  · rintro (hp | ⟨hab, hl⟩)
    · have := digit_lt (x := pa) (y := pb) (a := la) (b := lb) (M := 65536) hla hp
      exact Nat.add_lt_add_left (Nat.mul_lt_mul_of_pos_right this (by decide)) _
    · have := hcong hab; omega

theorem langPayload_lt {bs : List Nat} {lang : Nat} (h : Packable 5 bs) (hl : lang < 65536) :
    langPayload bs lang < S := by
  have := packN_lt 5 bs h.1
  unfold langPayload; simp only [S]; simp at this; omega

/-! ### Non-negative double magnitude (rule `id-double-inline`, PO-ID-DOUBLE)

A non-negative inline double is `+0`, `+∞`, or a normal value with IEEE biased
exponent `e ∈ [768, 1277]` (unbiased `[-255, 254]`) and 52-bit mantissa `m`.
Its value times `2^1075` is the natural number `(2^52 + m) * 2^e`. -/

inductive PosDbl where
  | zero
  | normal (e m : Nat)
  | inf
  deriving DecidableEq, Repr

/-- In the inline range of rule `id-double-inline`. -/
def PosDbl.Valid : PosDbl → Prop
  | .normal e m => 768 ≤ e ∧ e ≤ 1277 ∧ m < M52
  | _ => True

/-- Value order: `0 <` every normal `< ∞`, normals by `(2^52 + m) * 2^e`. -/
def PosDbl.Lt : PosDbl → PosDbl → Prop
  | .zero, .zero => False
  | .zero, _ => True
  | .normal _ _, .zero => False
  | .normal e₁ m₁, .normal e₂ m₂ => (M52 + m₁) * 2 ^ e₁ < (M52 + m₂) * 2 ^ e₂
  | .normal _ _, .inf => True
  | .inf, _ => False

/-- The 61-bit magnitude field: rebiased exponent `e - 767` (1..510; 0 for zero,
511 for infinity) followed by the 52 mantissa bits. -/
def PosDbl.mag : PosDbl → Nat
  | .zero => 0
  | .normal e m => (e - 767) * M52 + m
  | .inf => 511 * M52

theorem PosDbl.mag_lt_D {x : PosDbl} (h : x.Valid) : x.mag < D := by
  cases x with
  | zero => simp only [mag]; id_omega
  | inf => simp only [mag]; id_omega
  | normal e m => simp only [Valid] at h; simp only [mag]; id_omega

/-- A larger exponent outweighs any mantissa. -/
theorem scaled_lt_of_exp_lt {e₁ e₂ m₁ m₂ : Nat} (hm₁ : m₁ < M52) (he : e₁ < e₂) :
    (M52 + m₁) * 2 ^ e₁ < (M52 + m₂) * 2 ^ e₂ := by
  have hp : 0 < 2 ^ e₁ := Nat.two_pow_pos e₁
  have h1 : (M52 + m₁) * 2 ^ e₁ < (2 * M52) * 2 ^ e₁ :=
    Nat.mul_lt_mul_of_pos_right (by omega) hp
  have h2 : 2 ^ (e₁ + 1) ≤ 2 ^ e₂ := Nat.pow_le_pow_right (by decide) he
  have h3 : (2 * M52) * 2 ^ e₁ = M52 * 2 ^ (e₁ + 1) := by
    rw [Nat.pow_succ, Nat.mul_comm 2 M52, Nat.mul_assoc, Nat.mul_comm 2]
  have h4 : M52 * 2 ^ (e₁ + 1) ≤ (M52 + m₂) * 2 ^ e₂ :=
    Nat.mul_le_mul (Nat.le_add_right _ _) h2
  omega

theorem mag_normal_lt {e₁ m₁ e₂ m₂ : Nat} (he₁ : 768 ≤ e₁) (hm₁ : m₁ < M52) (hm₂ : m₂ < M52)
    (h : (M52 + m₁) * 2 ^ e₁ < (M52 + m₂) * 2 ^ e₂) :
    (e₁ - 767) * M52 + m₁ < (e₂ - 767) * M52 + m₂ := by
  rcases Nat.lt_trichotomy e₁ e₂ with he | rfl | he
  · have : (e₁ - 767 + 1) * M52 ≤ (e₂ - 767) * M52 := Nat.mul_le_mul_right _ (by omega)
    rw [Nat.add_mul, Nat.one_mul] at this
    omega
  · have := Nat.lt_of_mul_lt_mul_right h; omega
  · exact absurd h (Nat.lt_asymm (scaled_lt_of_exp_lt hm₂ he))

/-- PO-ID-DOUBLE: the magnitude field is strictly monotone in value order. -/
theorem PosDbl.mag_strictMono {x y : PosDbl} (hx : x.Valid) (hy : y.Valid) (h : x.Lt y) :
    x.mag < y.mag := by
  match x, y, hx, hy, h with
  | .zero, .normal e m, _, hy, _ => simp only [Valid] at hy; simp only [mag]; id_omega
  | .zero, .inf, _, _, _ => simp only [mag]; id_omega
  | .normal _ _, .normal _ _, hx, hy, h => exact mag_normal_lt hx.1 hx.2.2 hy.2.2 h
  | .normal e m, .inf, hx, _, _ => simp only [Valid] at hx; simp only [mag]; id_omega

theorem PosDbl.lt_trichotomy {x y : PosDbl} (hx : x.Valid) (hy : y.Valid) :
    x.Lt y ∨ x = y ∨ y.Lt x := by
  match x, y, hx, hy with
  | .zero, .zero, _, _ => exact Or.inr (Or.inl rfl)
  | .zero, .normal _ _, _, _ => exact Or.inl trivial
  | .zero, .inf, _, _ => exact Or.inl trivial
  | .normal _ _, .zero, _, _ => exact Or.inr (Or.inr trivial)
  | .normal e₁ m₁, .normal e₂ m₂, hx, hy =>
    simp only [Lt]
    rcases Nat.lt_trichotomy ((M52 + m₁) * 2 ^ e₁) ((M52 + m₂) * 2 ^ e₂) with h | h | h
    · exact Or.inl h
    · refine Or.inr (Or.inl ?_)
      rcases Nat.lt_trichotomy e₁ e₂ with he | rfl | he
      · exact absurd h (Nat.ne_of_lt (scaled_lt_of_exp_lt hx.2.2 he))
      · have := Nat.eq_of_mul_eq_mul_right (Nat.two_pow_pos e₁) h
        have : m₁ = m₂ := by omega
        rw [this]
      · exact absurd h.symm (Nat.ne_of_lt (scaled_lt_of_exp_lt hy.2.2 he))
    · exact Or.inr (Or.inr h)
  | .normal _ _, .inf, _, _ => exact Or.inl trivial
  | .inf, .zero, _, _ => exact Or.inr (Or.inr trivial)
  | .inf, .normal _ _, _, _ => exact Or.inr (Or.inr trivial)
  | .inf, .inf, _, _ => exact Or.inr (Or.inl rfl)

theorem PosDbl.lt_irrefl (x : PosDbl) : ¬ x.Lt x := by
  cases x <;> simp [Lt]

/-- Magnitude order is exactly value order on non-negative inline doubles. -/
theorem PosDbl.mag_lt_iff {x y : PosDbl} (hx : x.Valid) (hy : y.Valid) :
    x.mag < y.mag ↔ x.Lt y := by
  constructor
  · intro h
    rcases PosDbl.lt_trichotomy hx hy with h' | rfl | h'
    · exact h'
    · exact absurd h (Nat.lt_irrefl _)
    · exact absurd (PosDbl.mag_strictMono hy hx h') (Nat.lt_asymm h)
  · exact PosDbl.mag_strictMono hx hy

/-- Value order on signed inline doubles: every negative below every
non-negative (`-0.0 < +0.0` as distinct IDs); among negatives, larger magnitude
is smaller. -/
def SDblLt : Bool × PosDbl → Bool × PosDbl → Prop
  | (true, x₁), (true, x₂) => x₂.Lt x₁
  | (true, _), (false, _) => True
  | (false, _), (true, _) => False
  | (false, x₁), (false, x₂) => x₁.Lt x₂

/-- Full ID of an inline double from its sign and non-negative magnitude. -/
def encDblV (neg : Bool) (x : PosDbl) : Nat := encDbl neg x.mag

/-- PO-ID-DOUBLE, end to end: unsigned ID order is value order for every inline
double, negatives included. -/
theorem encDblV_strictMono {n₁ n₂ : Bool} {x₁ x₂ : PosDbl} (h₁ : x₁.Valid) (h₂ : x₂.Valid)
    (h : SDblLt (n₁, x₁) (n₂, x₂)) : encDblV n₁ x₁ < encDblV n₂ x₂ := by
  unfold encDblV
  apply encDbl_strictMono (PosDbl.mag_lt_D h₁) (PosDbl.mag_lt_D h₂)
  cases n₁ <;> cases n₂ <;> simp only [SDblLt] at h <;> simp only [DblLt]
  · exact PosDbl.mag_strictMono h₁ h₂ h
  · exact PosDbl.mag_strictMono h₂ h₁ h

/-- Every inline double is an inline-literal ID (tag at least `0x8`). -/
theorem encDblV_literal {n : Bool} {x : PosDbl} (h : x.Valid) : 8 ≤ kind (encDblV n x) :=
  encDbl_literal (PosDbl.mag_lt_D h)

end MetaCore.Id
