/-
  Oracle: runs the `MetaCore.Graphs.apply` model (statements, hyperedges, graph
  memberships, nesting) on a seeded random operation sequence and prints one
  JSON object per line. The Rust model-based test
  (`crates/graphlava/tests/refinement.rs`, task 11.4) replays the same
  operations against the real store and must produce the same results.

  Usage: lake exe oracle <seed> <count>

  ## Trace contract (JSON lines)

  Line 1, the schema:
    {"schema":{"multi":[7],"htypes":[{"id":0,"roles":[0,1],"required":[0,1]},
      {"id":1,"roles":[0,1,2],"required":[0]}]},"seed":S,"count":N}
  Predicate 7 is `multi`; every other predicate is unique. Hyperedge type 0
  declares roles 0 and 1, both required; type 1 declares roles 0, 1 and 2, only
  0 required. Every role has cardinality one.

  Terms: {"node":n} | {"lit":n} | {"stmt":i}. A triple is {"s":T,"p":p,"o":T}.
  Graph numbers are plain naturals: 0 is the default graph, 1..5 named graphs.
  IDs come from one counter shared by statements and hyperedges, starting at 1.

  One line per operation:
    {"op":"insert","triple":T,"graph":g,"expect":{"id":i}}
        existing non-multi triple: returns its ID and adds membership g (no-op if
        already a direct member); otherwise a new ID with membership {g}.
    {"op":"update","id":i,"o":TERM,"expect":{"ok":b}}
        keeps the ID and the memberships.
    {"op":"delete","id":i,"detach":b,"expect":E}
        `id` is a statement or a hyperedge. E is one of
        {"ok":true,"removed":[sorted ids]}   (statements and hyperedges removed)
        {"ok":false,"blocked":true}          (object guard or hyperedge guard)
        {"ok":false}                         (id is not live)
    {"op":"hedge","type":t,"graph":g,"roles":[[r,TERM],...],"expect":{"id":i}}
        or "expect":{"ok":false} when validation fails (missing required role,
        undeclared role, or a role named twice). Fillers are {"node":n} or
        {"stmt":i} of a live statement, never literals. An invalid hyperedge
        allocates no ID.
    {"op":"add_member","id":i,"graph":g,"expect":{"ok":b}}
        ok:false iff id is not a live statement.
    {"op":"remove_member","id":i,"graph":g,"expect":E}
        E is {"ok":true,"removed":[]} when only the membership went,
        {"ok":true,"removed":[sorted ids]} when g was the last membership and the
        plain (non-DETACH) cascade delete of id succeeded,
        {"ok":false,"blocked":true} when that cascade was blocked (nothing changes),
        {"ok":false} when id is not live or not a direct member of g.
    {"op":"nest","child":c,"parent":p,"expect":{"ok":b}}
        c, p in 1..5; ok:false iff c = p or p is already (transitively) under c;
        ok:true and no change if the edge exists.
    {"op":"unnest","child":c,"parent":p,"expect":{"ok":b}}
        ok:false iff the edge does not exist.
    {"op":"scope","graph":g,"direct":b,"expect":{"ids":[sorted statement ids]}}
        live statements that are direct members of g (direct) or of g or any graph
        transitively nested under g; each ID once. Hyperedges are never listed.

  Final line:
    {"final":[[id,TRIPLE],...],"hedges":[[id,{"type":t,"roles":[[r,TERM],...]}],...],
     "members":[[id,[sorted graphs]],...],"next":n}
  `final` lists live statements, `hedges` live hyperedges with roles in creation
  order, `members` every live statement with its direct graphs (all ascending
  by ID), `next` is the number of IDs ever allocated plus one.

  Every step is `MetaCore.Graphs.apply`, so `apply_inv` (with `OpLive`, which the
  generator guarantees) covers every state this trace passes through.
-/
import MetaCore
open MetaCore.Store
open MetaCore.Hyperedge (HType HEntry HStore Roles hids)
open MetaCore.Graphs

/-- Predicate 7 is declared `multi` in the trace schema; all others are unique. -/
def multiP (p : Nat) : Bool := p == 7

/-- The two hyperedge types of the trace schema. -/
def htypes : List HType := [⟨0, [0, 1], [0, 1]⟩, ⟨1, [0, 1, 2], [0]⟩]

def typesF (ty : Nat) : Option HType := htypes.find? (·.id == ty)

def lcg (x : Nat) : Nat := (x * 6364136223846793005 + 1442695040888963407) % 18446744073709551616

def natsJson (l : List Nat) : String := "[" ++ ",".intercalate (l.map toString) ++ "]"

def termJson : Term → String
  | .node n => "{\"node\":" ++ toString n ++ "}"
  | .lit n => "{\"lit\":" ++ toString n ++ "}"
  | .stmt i => "{\"stmt\":" ++ toString i ++ "}"

def tripleJson (t : Triple) : String :=
  "{\"s\":" ++ termJson t.s ++ ",\"p\":" ++ toString t.p ++ ",\"o\":" ++ termJson t.o ++ "}"

def rolesJson (rs : Roles) : String :=
  "[" ++ ",".intercalate (rs.map fun rf => "[" ++ toString rf.1 ++ "," ++ termJson rf.2 ++ "]") ++ "]"

def htypeJson (T : HType) : String :=
  "{\"id\":" ++ toString T.id ++ ",\"roles\":" ++ natsJson T.roles ++ ",\"required\":" ++
    natsJson T.required ++ "}"

def resJson : Res → String
  | .id i => "{\"id\":" ++ toString i ++ "}"
  | .ok b => "{\"ok\":" ++ toString b ++ "}"
  | .removed l => "{\"ok\":true,\"removed\":" ++ natsJson l ++ "}"
  | .blocked => "{\"ok\":false,\"blocked\":true}"
  | .ids l => "{\"ids\":" ++ natsJson l ++ "}"

def opJson : Op → String
  | .insert t g => "{\"op\":\"insert\",\"triple\":" ++ tripleJson t ++ ",\"graph\":" ++ toString g
  | .update id o => "{\"op\":\"update\",\"id\":" ++ toString id ++ ",\"o\":" ++ termJson o
  | .delete id d => "{\"op\":\"delete\",\"id\":" ++ toString id ++ ",\"detach\":" ++ toString d
  | .hedge ty g rs => "{\"op\":\"hedge\",\"type\":" ++ toString ty ++ ",\"graph\":" ++ toString g ++
      ",\"roles\":" ++ rolesJson rs
  | .addMember id g => "{\"op\":\"add_member\",\"id\":" ++ toString id ++ ",\"graph\":" ++ toString g
  | .removeMember id g =>
      "{\"op\":\"remove_member\",\"id\":" ++ toString id ++ ",\"graph\":" ++ toString g
  | .nest c p => "{\"op\":\"nest\",\"child\":" ++ toString c ++ ",\"parent\":" ++ toString p
  | .unnest c p => "{\"op\":\"unnest\",\"child\":" ++ toString c ++ ",\"parent\":" ++ toString p
  | .scope g d => "{\"op\":\"scope\",\"graph\":" ++ toString g ++ ",\"direct\":" ++ toString d

def pick (xs : List Nat) (r : Nat) : Nat :=
  match xs with
  | [] => r % 4
  | _ => xs.getD (r % xs.length) 0

/-- Statement terms only ever name live statements, so every generated store
satisfies `RefClosed` and every op satisfies `OpLive` (the precondition of
`apply_inv`). Objects may be statements too, which exercises the object guard of
ADR 0036. -/
def genTerm (st : Store) (r : Nat) : Term :=
  match r % 5, ids st with
  | 0, i :: is => .stmt ((i :: is).getD (r / 5 % (is.length + 1)) i)
  | 1, _ => .lit (r / 5 % 16)
  | _, _ => .node (r / 5 % 8)

/-- Rule `dm-term-positions`: a literal is never a subject. -/
def genSubject (st : Store) (r : Nat) : Term :=
  match genTerm st r with
  | .lit n => .node n
  | t => t

/-- Role fillers are nodes or live statements, never literals. -/
def genFiller (st : Store) (r : Nat) : Term := genSubject st r

def mkRoles (st : Store) : List Nat → Nat → Roles
  | [], _ => []
  | r :: rs, x => (r, genFiller st x) :: mkRoles st rs (lcg x)

/-- A hyperedge write: valid about two times in three; otherwise one of the
three shape errors of rule `he-type-declared`. Valid roles are listed in
declaration order. -/
def genHedge (st : Store) (r1 r2 r3 : Nat) : Nat × Roles :=
  let ty := r1 % 2
  let T := (typesF ty).getD ⟨ty, [], []⟩
  -- each optional role is present with probability 1/2
  let chosen := T.roles.filter (fun r => T.required.contains r || (r2 / 16 / (2 ^ r)) % 2 == 1)
  let rs := mkRoles st chosen r3
  let extra := genFiller st (lcg (lcg r3))
  match r2 % 9 with
  | 0 => (ty, rs.filter (fun rf => rf.1 != 0))                    -- missing required role 0
  | 1 => (ty, rs ++ [(T.roles.length + (r3 / 7 % 2), extra)])       -- undeclared role
  | 2 => (ty, rs ++ [(0, extra)])                                   -- role 0 named twice
  | _ => (ty, rs)

/-- High bits of an LCG state. The low bits of a power-of-two LCG are weak (bit 0
alternates), and `r % 30` already fixes the parity of `r`, so every choice below
that is not the op kind reads high bits. -/
def hi (x : Nat) : Nat := x / 4294967296

def genOp (w : World) (r : Nat) : Op :=
  let r1 := lcg r
  let r2 := lcg r1
  let r3 := lcg r2
  let st := w.hs.st
  let k := r % 30
  if k < 11 then
    let t : Triple := { s := genSubject st r1, p := r2 % 8, o := genTerm st (r2 / 8) }
    -- graph 0 about twice as often as each named graph
    let g := let x := hi r3 % 7; if x ≥ 6 then 0 else x
    .insert t g
  else if k < 16 then .update (pick (ids st) r1) (genTerm st r2)
  -- ADR 0036: about one delete in three is `DETACH`.
  else if k < 20 then .delete (pick (allIds w.hs) r1) (r2 % 3 == 0)
  else if k < 22 then
    let (ty, rs) := genHedge st (hi r1) r2 r3
    .hedge ty (hi r3 % 6) rs
  else if k < 24 then .addMember (pick (allIds w.hs) r1) (hi r2 % 6)
  else if k < 26 then
    let id := pick (ids st) r1
    -- mostly a graph the statement is in, so last memberships do get removed
    let g := if hi r2 % 4 == 0 then hi r3 % 6 else pick (w.mem id) (hi r3)
    .removeMember id g
  else if k < 28 then .nest (1 + hi r1 % 5) (1 + hi r2 % 5)
  else if k < 29 then
    match w.nest, hi r1 % 3 with
    | e :: es, 0 | e :: es, 1 =>
      let (c, p) := (e :: es).getD (hi r2 % (es.length + 1)) e; .unnest c p
    | _, _ => .unnest (1 + hi r2 % 5) (1 + hi r3 % 5)
  else .scope (hi r1 % 6) (hi r2 % 2 == 0)

def finalJson (w : World) : String :=
  let stmts := w.hs.st.stmts.reverse
  let es := stmts.map fun e => "[" ++ toString e.1 ++ "," ++ tripleJson e.2 ++ "]"
  let hs := w.hs.hedges.reverse.map fun (h : HEntry) =>
    "[" ++ toString h.id ++ ",{\"type\":" ++ toString h.ty ++ ",\"roles\":" ++ rolesJson h.roles ++ "}]"
  let ms := stmts.map fun e => "[" ++ toString e.1 ++ "," ++ natsJson ((w.mem e.1).mergeSort (· ≤ ·)) ++ "]"
  "{\"final\":[" ++ ",".intercalate es ++ "],\"hedges\":[" ++ ",".intercalate hs ++
    "],\"members\":[" ++ ",".intercalate ms ++ "],\"next\":" ++ toString w.hs.st.next ++ "}"

def main (args : List String) : IO UInt32 := do
  let seed := (args.getD 0 "1").toNat?.getD 1
  let count := (args.getD 1 "100").toNat?.getD 100
  IO.println ("{\"schema\":{\"multi\":[7],\"htypes\":[" ++ ",".intercalate (htypes.map htypeJson) ++
    "]},\"seed\":" ++ toString seed ++ ",\"count\":" ++ toString count ++ "}")
  let mut w := MetaCore.Graphs.empty
  let mut r := lcg seed
  for _ in [0:count] do
    let op := genOp w r
    let (w', res) := apply multiP typesF w op
    IO.println (opJson op ++ ",\"expect\":" ++ resJson res ++ "}")
    w := w'
    r := lcg r
  IO.println (finalJson w)
  return 0
