---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# One portable database file format with a version and feature flags

## Context and Problem Statement

Databases may be created on a desktop and opened on a phone, and the other way round. The on-disk format must either work everywhere or be converted per platform.

## Considered Options

<option id="portable" outcome="chosen">

Little-endian everywhere, no OS-specific fields, on top of redb's cross-platform file format. A `meta` record holds the format version and feature flags (orderings present, encoding v1 or v2). A build that sees an unknown flag refuses to open the file. A file is moved only when closed, or through a consistent snapshot export. No size cap in the format; phone planning size is about 1-5 GB (to be measured in the mobile benchmarks).

</option>

<option id="per-platform" outcome="rejected">

Platform-tuned formats with export/import: more freedom per platform, but no file copy and more code paths to test.

</option>

## Decision Outcome

Chosen option: "portable", because a file copy between desktop and phone is a valuable property and the cost is only discipline about format changes.

### Consequences

<consequence kind="good">

Copy a closed file between all five platforms; refusal on unknown flags avoids silent misreads.

</consequence>

<consequence kind="bad">

Every on-disk change after v0 needs a migration path. The format must be frozen deliberately. Platform-specific tuning must stay outside the file.

</consequence>

## More Information

Resolves handover open question 7. Related: [ADR-0002](0002-redb-single-owner-process.md).
