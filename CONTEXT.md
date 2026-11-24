# meta-core

A mutable, current-state-only metagraph database in Rust, tuned for agent-memory workloads.

## Language

**Owner process**:
The single OS process that has a database file open. All other clients reach the database through it.
_Avoid_: server, daemon (a daemon is one way to host the owner process)

**Statement**:
A subject–predicate–object fact with a stable 64-bit ID. Its object can change in place without changing the ID.
_Avoid_: triple, edge, fact (unless you mean the derived kind)

**Graph**:
A node that groups statements. Graphs can contain other graphs.
_Avoid_: context, namespace, dataset

**Partitioned graph**:
A graph large enough to have its own tables in the database file, for isolation and fast drop.
_Avoid_: shard, tenant store

**Multi predicate**:
A predicate declared to allow several statements with the same subject and object. All other predicates allow at most one.
_Avoid_: multi-edge type, duplicate edge

**Functional predicate**:
A predicate declared to allow at most one object per subject. Writing a new object updates the existing statement in place, keeping its ID.
_Avoid_: single-valued edge, property (that is an inline property)

**Inline property**:
A statement whose object is a literal, stored in its subject's property record instead of the orderings.
_Avoid_: attribute, literal edge

**Owned literal**:
A long literal that belongs to one property slot or statement instead of being interned. It is deleted with its slot; equal texts in two slots are two owned literals that compare equal.
_Avoid_: blob, large object

**Promotion**:
Moving an inline property into the orderings with its own statement ID when something annotates it.
_Avoid_: reification (that is the RDF-level term for the annotation itself)

**Scope**:
The set of statements a query sees through a graph clause: the graph and, by default, all graphs nested under it.
_Avoid_: membership (that is the single `:in` relation, not the closure)

**Derived fact**:
A statement produced by a rule instead of being written directly. It has no statement ID until something annotates it.
_Avoid_: inferred triple, materialized triple

**Hyperedge**:
A single record with a type and several named roles, each filled by a node. It is stored once, not as one statement per role.
_Avoid_: n-ary edge, relation instance

**Chunk**:
A leased block of consecutive IDs owned by one graph and one kind of item.
_Avoid_: range, page, segment

**Durability level**:
How strongly a transaction is guaranteed to survive a crash at the moment it is acknowledged: `sync`, `ordered` or `relaxed`.
_Avoid_: fsync mode, consistency level

**Batch**:
The write requests the owner process runs inside one engine write transaction and commits together. A batch becomes visible when it commits.
_Avoid_: group (as a noun), bulk

**Write request**:
A complete write (SPARQL Update, Datalog program or list of core operations) that a client submits for the writer to run in one batch, without holding the writer between round trips.
_Avoid_: session, remote transaction
