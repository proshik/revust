---
description: Regression coverage
---
## Regression coverage

Assess whether the tests distinguish intended behavior from plausible faults. Inspect assertions, isolation and failure cases. Explain a specific regression the suite misses, or a test that reports success without exercising its claimed boundary.

Where affected, require a regression that exercises the original scenario with meaningful assertions. Inspect isolation and cleanup after failure, and whether failure of a second mutation proves rollback of the first. Look for relevant coverage of concurrent SQL operations, migration of existing data, compatibility across versions and failure/recovery boundaries. Messaging regressions may require exercising commit/publication divergence, unknown confirms, duplicate delivery, database-effect/ack gaps, ordering and replay. Identify the plausible fault the assertion would detect, and distinguish realistic boundary coverage from mocks that assume the guarantee being tested; do not demand every scenario regardless of scope.
