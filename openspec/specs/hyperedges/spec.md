# hyperedges Specification

## Purpose

Typed n-ary hyperedges with named roles, stored once per edge and exposed to queries as role triples. Design: `tg.md/08-hyperedges.md`.

## Requirements

### Requirement: Typed hyperedges with roles
The system SHALL store a hyperedge as one item with a type and a set of named roles, each filled by a node, and SHALL validate arity and roles against the type's definition in the meta-model.

Formal: `MetaCore.Hyperedge.shapeB_iff`, `MetaCore.Hyperedge.satisfies_arity`, `MetaCore.Hyperedge.satisfies_fixed_arity`, `MetaCore.Hyperedge.create_some`, `MetaCore.Hyperedge.create_none_iff`

#### Scenario: Create a meeting hyperedge
- **Level**: e2e
- **WHEN** type `:Meeting` declares roles `:agent`, `:target` and the literal attribute `:when`, and a client creates a meeting with all three
- **THEN** a hyperedge ID is returned and the two roles and the attribute are readable

#### Scenario: Missing role rejected
- **Level**: e2e
- **WHEN** a `:Meeting` is created without `:target` and the type marks it required
- **THEN** the write fails

### Requirement: Literal attributes of hyperedges
The system SHALL store a literal value of a hyperedge, such as a time, as an inline property of the hyperedge node, never as a role filler.

Formal: `MetaCore.Hyperedge.create_some` (no filler is a literal); inline-property routing is outside the model

#### Scenario: Meeting time is an attribute
- **Level**: e2e
- **WHEN** a meeting is created with `:when "2026-11-24T10:00:00Z"^^xsd:dateTime`
- **THEN** `(e, :when, "2026-11-24T10:00:00Z"^^xsd:dateTime)` is returned by queries, and no incidence entry holds the time

### Requirement: Role triples are queryable
The system SHALL expose each role filler as a triple `(edge, role, node)` to every query surface, without storing one statement per role.

Formal: none — query surfaces are outside the model

#### Scenario: Find meetings of an agent
- **Level**: e2e
- **WHEN** a query asks for `?e :agent :alice . ?e a :Meeting`
- **THEN** it returns every meeting in which `:alice` fills `:agent`

### Requirement: Hyperedges in graphs and type scans
The system SHALL place each hyperedge in a home graph and SHALL support listing all hyperedges of a type without scanning other items.

Formal: none — chunk layout and type scans are outside the model

#### Scenario: Scope and type together
- **Level**: e2e
- **WHEN** meetings exist in `:g1` and `:g2` and a query asks for meetings in `:g1`
- **THEN** only the meetings of `:g1` are returned

### Requirement: Annotating a role
The system SHALL let a client annotate one role filling of a hyperedge, giving that role a statement ID.

Formal: none — promoted role edges are outside the model (partial note in tg.md/20-proofs.md)

#### Scenario: Confidence on a role
- **Level**: e2e
- **WHEN** a client annotates `(e, :target, :bob)` with `:confidence 0.7`
- **THEN** the role filling has a statement ID carrying that annotation, and the hyperedge is otherwise unchanged

### Requirement: Hyperedge roles block delete
A delete of an item that fills a role of a hyperedge SHALL fail unless the caller asks to detach, which removes that hyperedge in the same transaction.

Formal: `MetaCore.Hyperedge.cascadeDelete_correct`, `MetaCore.Hyperedge.cascadeDelete_none_iff`, `MetaCore.Hyperedge.cascadeDelete_detach_some`, `MetaCore.Hyperedge.cascadeDelete_hedge_blocked`

#### Scenario: Hyperedge role blocks delete
- **Level**: e2e
- **WHEN** a hyperedge has `#42` in a role and `#42` is deleted without detach
- **THEN** the delete fails and nothing changes

#### Scenario: Detach removes the hyperedge
- **Level**: e2e
- **WHEN** a hyperedge has `#42` in a role and `#42` is deleted with detach
- **THEN** `#42` and the hyperedge are gone, and the other role fillers remain
