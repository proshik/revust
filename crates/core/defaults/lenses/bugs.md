---
description: Runtime correctness
---
## Runtime correctness

Trace an accepted input through the changed path to its observable result. Examine boundary values, ordering, lifetime and failure recovery. Report the exact transition that violates the supported behavior.

Trace transaction boundaries and SQL concurrency: lost updates, isolation, locks and uniqueness under competing operations. Where relevant, distinguish absent, null, empty, default and false values through PATCH, inheritance and reset semantics. Check query filters and pagination, time/deadline/timezone and clock assumptions, and stale caches or delayed responses after state changes against supported behavior. Follow independent effects of a business operation: a database commit and Kafka/Rabbit publication may diverge in either direction. Establish the required delivery guarantee before alleging loss or duplication; inspect broker confirmation versus local enqueue, unknown delivery outcomes and the consumer's database effect versus acknowledgement or offset commit.
