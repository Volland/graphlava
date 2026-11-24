/-
  Oracle: runs the `MetaCore.Store` model on a seeded random operation
  sequence and prints one JSON object per line. The Rust model-based test
  (`crates/meta-core/tests/refinement.rs`, task 11.4) replays the same
  operations against the real store and must produce the same results.

  Usage: lake exe oracle <seed> <count>
-/
import MetaCore
open MetaCore.Store

/-- Predicate 7 is declared `multi` in the trace schema; all others are unique. -/
def multiP (p : Nat) : Bool := p == 7

def lcg (x : Nat) : Nat := (x * 6364136223846793005 + 1442695040888963407) % 18446744073709551616

def termJson : Term → String
  | .node n => "{\"node\":" ++ toString n ++ "}"
  | .lit n => "{\"lit\":" ++ toString n ++ "}"
  | .stmt i => "{\"stmt\":" ++ toString i ++ "}"

def tripleJson (t : Triple) : String :=
  "{\"s\":" ++ termJson t.s ++ ",\"p\":" ++ toString t.p ++ ",\"o\":" ++ termJson t.o ++ "}"

def pick (xs : List Nat) (r : Nat) : Nat :=
  match xs with
  | [] => r % 4
  | _ => xs.getD (r % xs.length) 0

/-- Statement terms only ever name live statements, so every generated store
satisfies `RefClosed` (the precondition of `cascadeDelete_correct`). -/
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

def step (st : Store) (r : Nat) : Store × String :=
  let r1 := lcg r
  let r2 := lcg r1
  match r % 10 with
  | 0 | 1 | 2 | 3 | 4 =>
    let t : Triple := { s := genSubject st r1, p := r2 % 8, o := genTerm st (r2 / 8) }
    let (st', id) := insert multiP st t
    (st', "{\"op\":\"insert\",\"triple\":" ++ tripleJson t ++ ",\"expect\":{\"id\":" ++ toString id ++ "}}")
  | 5 | 6 | 7 =>
    let id := pick (ids st) r1
    let o := genTerm st r2
    match update multiP st id o with
    | some st' => (st', "{\"op\":\"update\",\"id\":" ++ toString id ++ ",\"o\":" ++ termJson o ++ ",\"expect\":{\"ok\":true}}")
    | none => (st, "{\"op\":\"update\",\"id\":" ++ toString id ++ ",\"o\":" ++ termJson o ++ ",\"expect\":{\"ok\":false}}")
  | _ =>
    let id := pick (ids st) r1
    if (ids st).contains id then
      match cascadeDelete st id with
      | some st' =>
        let gone := (ids st).filter (fun i => !(ids st').contains i)
        (st', "{\"op\":\"delete\",\"id\":" ++ toString id ++ ",\"expect\":{\"ok\":true,\"removed\":" ++ toString gone ++ "}}")
      | none => (st, "{\"op\":\"delete\",\"id\":" ++ toString id ++ ",\"expect\":{\"ok\":false,\"fuel\":true}}")
    else (st, "{\"op\":\"delete\",\"id\":" ++ toString id ++ ",\"expect\":{\"ok\":false}}")

def finalJson (st : Store) : String :=
  let es := (st.stmts.reverse.map fun e => "[" ++ toString e.1 ++ "," ++ tripleJson e.2 ++ "]")
  "{\"final\":[" ++ ",".intercalate es ++ "],\"next\":" ++ toString st.next ++ "}"

def main (args : List String) : IO UInt32 := do
  let seed := (args.getD 0 "1").toNat?.getD 1
  let count := (args.getD 1 "100").toNat?.getD 100
  IO.println ("{\"schema\":{\"multi\":[7]},\"seed\":" ++ toString seed ++ ",\"count\":" ++ toString count ++ "}")
  let mut st := empty
  let mut r := lcg seed
  for _ in [0:count] do
    let (st', line) := step st r
    IO.println line
    st := st'
    r := lcg r
  IO.println (finalJson st)
  return 0
