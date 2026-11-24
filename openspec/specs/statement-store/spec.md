# statement-store Specification

## Purpose

How statements are stored, identified, overwritten, annotated and deleted, including literal-valued properties stored inline per node. Design: `tg.md/01-data-model.md`, `tg.md/04-physical-layout.md`, `tg.md/06-transactions-durability.md`.

## Requirements

### Requirement: Statement identity under update
The system SHALL keep a statement's ID when its object is changed in place, and annotations on that statement SHALL stay attached.

Formal: `MetaCore.Store.update_ids`, `MetaCore.Store.update_lookup`, `MetaCore.Store.update_wf`

#### Scenario: Overwrite keeps ID and annotations
- **Level**: e2e
- **WHEN** statement `#42 = (:alice, :worksAt, :acme)` carries annotation `(#42, :confidence, 0.9)` and its object is updated in place to `:globex`
- **THEN** `#42` now reads `(:alice, :worksAt, :globex)` and the annotation still has subject `#42`

### Requirement: Unique statements by default
The system SHALL hold at most one statement per `(s, p, o)` unless the predicate is declared `multi` in the meta-model, in which case several statements with distinct IDs MAY share `(s, p, o)`.

Formal: `MetaCore.Store.insert_idempotent`, `MetaCore.Store.insert_wf`, `MetaCore.Store.insert_multi_distinct`

#### Scenario: Repeated insert is idempotent
- **Level**: e2e
- **WHEN** `(:a, :knows, :b)` is inserted twice on a non-multi predicate
- **THEN** one statement exists and both inserts report its ID

#### Scenario: Multi predicate keeps parallel statements
- **Level**: e2e
- **WHEN** `:transfer` is declared `multi` and `(:a, :transfer, :b)` is inserted twice
- **THEN** two statements with different IDs exist

### Requirement: Functional predicates update in place
The system SHALL let the meta-model declare a predicate `functional`; writing `(s, p, o')` when `(s, p, o)` exists on a functional predicate SHALL update that statement's object in place and keep its ID and annotations.

Formal: `MetaCore.Store.update_preserves_others` (the functional flag is not modelled)

#### Scenario: Set on a functional predicate keeps annotations
- **Level**: e2e
- **WHEN** `:currentLocation` is functional, `#42 = (:alice, :currentLocation, :paris)` carries `(#42, :source, :gps)`, and `(:alice, :currentLocation, :rome)` is written
- **THEN** `#42` reads `(:alice, :currentLocation, :rome)` and the annotation still has subject `#42`

#### Scenario: Compare-and-set update
- **Level**: e2e
- **WHEN** a client updates `#42` to `:berlin` with expected object `:paris` after another client has changed it to `:rome`
- **THEN** the update fails with a conflict and `#42` still reads `:rome`

### Requirement: Inline literal properties
The system SHALL store statements whose object is a literal in their subject's property record and SHALL expose them to queries exactly like other statements.

Formal: none — inline routing lies outside the model; covered at levels e2e and property

#### Scenario: Literal property is queryable as a triple
- **Level**: e2e
- **WHEN** `(:alice, :age, 42)` is written
- **THEN** a query for `?x :age ?v` returns `:alice` and `42`

#### Scenario: Literal predicate scan reads only nodes that have the predicate
- **Level**: e2e
- **WHEN** 10^6 nodes exist, 100 of them have `:age`, and a query asks for `?x :age ?v` with no value index
- **THEN** the query reads 100 property records, found through the predicate's node list, not 10^6

#### Scenario: Value lookup works without a declared index
- **Level**: e2e
- **WHEN** a query asks for `?x :email "a@b.c"` with no value index declared
- **THEN** it returns the correct subjects

### Requirement: Promotion on annotation
The system SHALL give an inline property a statement ID when something annotates it, and SHALL keep that ID on later value updates; a promoted property is never demoted.

Formal: none — promotion lies outside the model; covered at levels e2e and refinement

#### Scenario: Annotating an inline property
- **Level**: e2e
- **WHEN** a client annotates `(:alice, :age, 42)` with `:source :doc5`
- **THEN** the property gets a statement ID that is the subject of `(id, :source, :doc5)`

#### Scenario: Promoted property keeps ID on overwrite
- **Level**: e2e
- **WHEN** the promoted `:age` of `:alice` is overwritten with 43
- **THEN** its statement ID is unchanged and the annotation still applies

### Requirement: Cascade delete of annotations
The system SHALL delete, in the same transaction, every statement whose subject is a deleted statement, transitively; a delete SHALL fail when the object position of another statement references the deleted item unless the caller asks to detach, which removes the referring statement as well.

Formal: `MetaCore.Store.cascadeDelete_correct`, `MetaCore.Store.cascadeDelete_none_iff`, `MetaCore.Store.closure_closed`

#### Scenario: Annotation chain is removed
- **Level**: e2e
- **WHEN** `#42` is annotated by `#77` and `#77` by `#90`, and `#42` is deleted
- **THEN** `#42`, `#77` and `#90` no longer exist

#### Scenario: Object-position reference blocks delete
- **Level**: e2e
- **WHEN** `(:note, :about, #42)` exists and `#42` is deleted without detach
- **THEN** the delete fails and nothing changes; with detach, both `#42` and the `:about` statement are gone

#### Scenario: Keep annotations on request
- **Level**: e2e
- **WHEN** `#42` is deleted with keep-annotations
- **THEN** `#77` and `#90` remain and are reachable by ID

### Requirement: Index coverage of base orderings
The system SHALL answer every triple pattern with one or two bound positions through an index range on the always-present orderings, without a full scan.

Formal: none — physical layout; covered by the invariant checker at level e2e

#### Scenario: Object-bound lookup
- **Level**: e2e
- **WHEN** a query asks for `?s ?p :acme`
- **THEN** it is answered from an index range keyed by `:acme`
