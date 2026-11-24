# Spec Delta

## Purpose

How statements belong to graphs, how graphs nest, and what a graph-scoped query sees. Design: `tg.md/09-graphs.md`, `tg.md/11-sparql.md`.

## ADDED Requirements

### Requirement: Home graph membership
The system SHALL place every statement in exactly one home graph at creation and MAY record extra memberships; membership SHALL be exposed to queries as `:in` triples without storing a statement per membership.

#### Scenario: Statement in its home graph
- **WHEN** a statement is inserted into graph `:g1`
- **THEN** `(stmt, :in, :g1)` is visible to queries

#### Scenario: Extra membership
- **WHEN** an existing statement of `:g1` is also added to `:g2`
- **THEN** it is visible in both `:g1` and `:g2` scopes

### Requirement: One statement in several graphs
The system SHALL treat a triple asserted in several graphs as one statement with one membership per graph: inserting an existing triple into a graph SHALL add that membership, and deleting it inside a graph SHALL remove only that membership; the statement SHALL be deleted when its last membership is removed.

#### Scenario: Insert into a second graph adds a membership
- **WHEN** `(:alice, :knows, :bob)` exists in `:ep1` with ID `#42` and is inserted into `:ep2`
- **THEN** `#42` is visible in both `:ep1` and `:ep2` and no new statement is created

#### Scenario: Delete in one graph keeps the other
- **WHEN** `#42` is a member of `:ep1` and `:ep2` and is deleted inside `:ep1`
- **THEN** `#42` and its annotations remain, visible only in `:ep2`

#### Scenario: Per-graph metadata on memberships
- **WHEN** a client annotates `<< #42 :in :ep1 >>` with confidence 0.9 and `<< #42 :in :ep2 >>` with confidence 0.6
- **THEN** each membership returns its own confidence

### Requirement: Transitive graph scope by default
A query scoped to a graph SHALL see the statements of that graph and of every graph nested under it; a direct scope SHALL see only the graph's own members.

#### Scenario: Parent scope includes children
- **WHEN** `:episode1 :in :agent` holds statement S and a query is scoped to `:agent`
- **THEN** S is in the result

#### Scenario: Direct scope excludes children
- **WHEN** the same query uses direct scope on `:agent`
- **THEN** S is not in the result

#### Scenario: Shared child counts once
- **WHEN** `:episode1` is nested under both `:agent` and `:project`, and `:project :in :agent`
- **THEN** a query scoped to `:agent` returns each statement of `:episode1` once

### Requirement: Acyclic nesting
The system SHALL allow a graph to have several parents and SHALL reject any nesting that would create a cycle or exceed the configured depth limit (default 64).

#### Scenario: Cycle rejected
- **WHEN** `:a :in :b` exists and a client adds `:b :in :a`
- **THEN** the write fails and nesting is unchanged

### Requirement: Annotatable membership
The system SHALL let a client annotate a membership `(stmt, :in, g)`, giving it a statement ID at that moment.

#### Scenario: Annotating membership
- **WHEN** a client states that S was added to `:g1` at 10:42
- **THEN** a membership statement with an ID exists and carries that annotation

### Requirement: Graph drop
Dropping a graph SHALL remove every membership in it and its nesting edges, SHALL delete the statements left without any membership, and SHALL keep statements that are still members of another graph; descendants SHALL survive unless the drop is recursive.

#### Scenario: Plain drop keeps children
- **WHEN** `:episode1 :in :agent` and `:agent` is dropped
- **THEN** `:agent`'s own statements are gone and `:episode1` with its statements remains as a root graph

#### Scenario: Drop keeps statements shared with another graph
- **WHEN** `#42` is in `:ep1` (home) and `:ep2`, and `:ep1` is dropped
- **THEN** `#42` remains, visible in `:ep2` and not in `:ep1`

#### Scenario: Recursive drop
- **WHEN** `:agent` is dropped recursively
- **THEN** `:episode1` and its statements are gone too, unless `:episode1` has another surviving parent
