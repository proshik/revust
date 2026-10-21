---
description: Failure-oriented investigation
---
## Failure-oriented investigation

Construct realistic counterexamples to the happy path. Consider malformed input, partial completion, cancellation, races and trust boundaries. Reject scenarios excluded by the actual contract and document any prerequisite for exploitation.

Probe relevant tenant/auth boundaries, malformed or oversized inputs, exhausted limits and resource leaks. Challenge partial completion and cancellation with realistic crash windows, retry/backoff, poison messages, DLQ/replay and backlog growth. For Kafka/Rabbit consumers and publishers, seek counterexamples involving duplicate delivery, concurrent claims, reordering and ack/offset timing; show why existing idempotency, partitioning or recovery mechanisms do not exclude the scenario before reporting it.
