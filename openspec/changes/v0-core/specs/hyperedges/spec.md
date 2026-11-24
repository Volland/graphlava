# Spec Delta

## Purpose

Typed n-ary hyperedges with named roles, stored once per edge and exposed to queries as role triples. Design: `tg.md/08-hyperedges.md`.

## ADDED Requirements

### Requirement: Typed hyperedges with roles
The system SHALL store a hyperedge as one item with a type and a set of named roles, each filled by a node, and SHALL validate arity and roles against the type's definition in the meta-model.

#### Scenario: Create a meeting hyperedge
- **WHEN** type `:Meeting` declares roles `:agent`, `:target` and the literal attribute `:when`, and a client creates a meeting with all three
- **THEN** a hyperedge ID is returned and the two roles and the attribute are readable

#### Scenario: Missing role rejected
- **WHEN** a `:Meeting` is created without `:target` and the type marks it required
- **THEN** the write fails

### Requirement: Literal attributes of hyperedges
The system SHALL store a literal value of a hyperedge, such as a time, as an inline property of the hyperedge node, never as a role filler.

#### Scenario: Meeting time is an attribute
- **WHEN** a meeting is created with `:when "2026-11-24T10:00:00Z"^^xsd:dateTime`
- **THEN** `(e, :when, "2026-11-24T10:00:00Z"^^xsd:dateTime)` is returned by queries, and no incidence entry holds the time

### Requirement: Role triples are queryable
The system SHALL expose each role filler as a triple `(edge, role, node)` to every query surface, without storing one statement per role.

#### Scenario: Find meetings of an agent
- **WHEN** a query asks for `?e :agent :alice . ?e a :Meeting`
- **THEN** it returns every meeting in which `:alice` fills `:agent`

### Requirement: Hyperedges in graphs and type scans
The system SHALL place each hyperedge in a home graph and SHALL support listing all hyperedges of a type without scanning other items.

#### Scenario: Scope and type together
- **WHEN** meetings exist in `:g1` and `:g2` and a query asks for meetings in `:g1`
- **THEN** only the meetings of `:g1` are returned

### Requirement: Annotating a role
The system SHALL let a client annotate one role filling of a hyperedge, giving that role a statement ID.

#### Scenario: Confidence on a role
- **WHEN** a client annotates `(e, :target, :bob)` with `:confidence 0.7`
- **THEN** the role filling has a statement ID carrying that annotation, and the hyperedge is otherwise unchanged
