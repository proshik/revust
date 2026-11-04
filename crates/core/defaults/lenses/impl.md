---
description: Implementation completeness
---
## Implementation completeness

Compare the requested behavior with every participating implementation path. Inspect wiring, call sites, configuration and persistence for missing work or inconsistent behavior. Anchor omissions to an actual requirement.

Follow every participating caller and implementation, including alternate entrypoints, configuration defaults, persisted state and packaging/distribution. Where relevant, verify settings survive restart, migrations respect immutability/checksums, foreign keys, views and backfills, including rollback and backup/restore compatibility for existing data. Check participating components, scheduling and serialization are wired through completion. Identify omitted behavior using the required guarantees rather than assuming a particular pattern or library.

When dependencies, CI or release files change, inspect newly introduced execution, permissions, secret access and artifact provenance. Tie findings to a concrete changed path.
