---
tgs: "0.4"
schemas:
  OpenQuestion:
    description: A design question that is not decided yet, with its options and a recommendation, until a Decision resolves it.
    properties:
      status: {values: [open, decided, dropped], default: open, description: Where the question is in its life.}
      owner: {kind: text, description: Who decides.}
      gate: {values: [v0, v1, v2, none], description: The release that cannot be finished until this is decided.}
    elements:
      option:
        description: One possible answer, with its trade-off.
        attributes:
          id: {description: "Stable address, linked as file#^id."}
          outcome: {values: [chosen, rejected, deferred, recommended], description: What happened to this option; recommended marks the proposed answer.}
    visualization: {color: "#ef4444", shape: octagon}
  Benchmark:
    description: A measurement that must be run before a performance claim is trusted, with the workload, the metric and the pass line.
    properties:
      status: {values: [planned, running, done], default: planned, description: Where the measurement is.}
      gate: {values: [v0, v1, v2, none], description: The release the result gates.}
    elements:
      metric: A number to record, with its unit.
      target: The pass line, or "to be measured" when no number is known yet.
    visualization: {color: "#22c55e", shape: round-tag}
edgeTypes:
  follows_from:
    description: A requirement or concept exists because of this decision; the decision is its reason.
    from: [Requirement, Concept, Constraint]
    to: Decision
    properties:
      note: text
  resolved_by:
    description: The decision that answers an open question.
    from: OpenQuestion
    to: Decision
    properties:
      date: date
  blocks:
    description: An open question that must be decided before this requirement, decision or change can be finished.
    from: OpenQuestion
    to: [Requirement, Decision, Change]
    properties:
      note: text
  measured_by:
    description: A benchmark that checks a performance claim in this requirement or decision.
    from: [Requirement, Decision]
    to: Benchmark
    properties:
      note: text
---
# meta-core Ontology Schema

The project's own vocabulary on top of the code ontology: `OpenQuestion` and `Benchmark` types, and the edges `follows_from`, `resolved_by`, `blocks` and `measured_by`. They answer two questions the code ontology cannot: which design questions are still open and what they block, and which performance claims still need a measurement.
