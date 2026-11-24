# Spec Delta

## Purpose

Datalog as a standalone query surface over the same data and planner as SPARQL, with Nemo-compatible syntax, stratified negation and on-demand evaluation. Design: `tg.md/13-datalog.md`, `tg.md/10-ir.md`.

## ADDED Requirements

### Requirement: Standalone Datalog programs
The system SHALL accept a Datalog program with rules and goals as a query in its own right, run it in a read or write transaction, and return rows like a SPARQL query.

#### Scenario: Transitive closure query
- **WHEN** a program defines `reach(X,Y) :- triple(X, :knows, Y). reach(X,Z) :- reach(X,Y), triple(Y, :knows, Z).` and asks for `reach(:a, ?y)`
- **THEN** every node reachable from `:a` through `:knows` is returned once

### Requirement: Nemo-compatible syntax
The system SHALL parse rules in Nemo's syntax for the features it supports, and SHALL report an error naming the construct for anything it does not support.

#### Scenario: Unsupported construct
- **WHEN** a program uses a Nemo feature outside the supported subset
- **THEN** the program is rejected with an error naming that feature

### Requirement: Stratified negation and aggregation
The system SHALL evaluate negation and aggregation in stratified programs and SHALL reject programs that are not stratifiable.

#### Scenario: Unstratifiable program
- **WHEN** a program contains `p(X) :- q(X), not p(X).`
- **THEN** it is rejected with an error naming the cycle through negation

### Requirement: On-demand evaluation
Rules without an annotation SHALL be evaluated per query from the current snapshot, and their results SHALL NOT be stored.

#### Scenario: Results follow the data
- **WHEN** a goal is evaluated, an underlying statement is overwritten, and the goal is evaluated again
- **THEN** the second answer reflects the overwrite

### Requirement: Metagraph access from Datalog
Datalog programs SHALL see statement IDs, inline properties, hyperedge roles and graph membership as relations, the same ones SPARQL sees.

#### Scenario: Rule over annotations
- **WHEN** a rule selects statements whose `:confidence` annotation is above 0.8
- **THEN** it returns those statement IDs and their triples
