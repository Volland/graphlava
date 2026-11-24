# Spec Delta

## Purpose

How every term in a meta-core database gets one stable 64-bit identity, how small literals live inside that identity in value order, and how unused terms are reclaimed. Design: `tg.md/02-id-encoding.md`, `tg.md/03-dictionary.md`.

## ADDED Requirements

### Requirement: Stable term identity
The system SHALL give every IRI, blank node, literal, statement, graph and hyperedge exactly one 64-bit ID whose top four bits name its kind, and SHALL never reuse an ID after its item is deleted.

#### Scenario: Same term interns to the same ID
- **WHEN** the IRI `<http://ex.org/alice>` is written in two separate transactions
- **THEN** both writes resolve to the same ID

#### Scenario: Deleted IDs are not reused
- **WHEN** a statement is deleted and a new statement is inserted afterwards
- **THEN** the new statement's ID differs from every ID ever allocated before, including the deleted one

#### Scenario: Kind is readable from the ID
- **WHEN** a client receives an ID from a query
- **THEN** the client can tell from the ID alone whether it is an IRI, blank node, literal, statement, graph or hyperedge

### Requirement: Order-preserving inline literals
The system SHALL store booleans, integers in [-2^55, 2^55-1], dates, dateTimes and strings of at most 7 UTF-8 bytes inside the ID, encoded so that unsigned ID order equals value order within each datatype.

#### Scenario: Integer range filter uses value order
- **WHEN** integers -5, 3 and 1000 are stored and a filter asks for values greater than 0
- **THEN** the results are 3 and 1000, found by a key range without decoding terms

#### Scenario: Values outside the inline range stay exact
- **WHEN** the integer 2^60 is stored and read back
- **THEN** the returned literal equals 2^60 with datatype `xsd:integer`

### Requirement: Canonical literal forms
The system SHALL canonicalise literals before storing them, so that lexical variants of the same value receive the same ID.

#### Scenario: Leading zeros are folded
- **WHEN** `"01"^^xsd:integer` and `"1"^^xsd:integer` are both inserted as objects of `(:a, :n, _)`
- **THEN** one statement exists and its object reads back as `"1"^^xsd:integer`

#### Scenario: Plain and xsd:string are one term
- **WHEN** `"hi"` and `"hi"^^xsd:string` are written
- **THEN** both resolve to the same ID

### Requirement: Dictionary reclamation without write cost
The system SHALL reclaim dictionary terms that no stored data references through a background sweep, and SHALL NOT update reference counts on the write path.

#### Scenario: Unreferenced term is reclaimed
- **WHEN** the only statement using a long literal is deleted and a sweep runs after the safety window
- **THEN** the literal is no longer in the dictionary

#### Scenario: Recently interned term survives a sweep
- **WHEN** a sweep runs while a term interned less than the safety window ago has no references yet
- **THEN** the term is kept
