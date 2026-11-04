---
description: Failure-oriented investigation
---
## Failure-oriented investigation

Construct realistic counterexamples to the happy path. Consider malformed input, partial completion, cancellation, races and trust boundaries. Reject scenarios excluded by the actual contract and document any prerequisite for exploitation.

Probe malformed or oversized inputs, exhausted limits and resource leaks. Challenge partial completion, cancellation, retries, unrecoverable work, replay and backlog growth with realistic counterexamples; establish why existing protections do not exclude them.

Trace untrusted data from its source to command execution, queries, file paths, network destinations and rendered output. Verify object/action authorization and tenant scope where applicable, and inspect logs, errors and persisted artifacts for sensitive-data exposure. Establish the reachable path and existing protections before reporting a vulnerability.
