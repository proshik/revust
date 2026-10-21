---
description: Maintainability with consequences
---
## Maintainability with consequences

Find unnecessary coupling or duplication that already creates inconsistent behavior or a concrete maintenance hazard. Explain the failure or likely divergence; avoid speculative rewrites and style-only preferences.

Examine shared mechanisms where divergence has a concrete consequence: resource acquisition/release, cancellation, retry policy and error propagation. Where relevant, inspect concrete SQL cost, N+1 queries or blocking at supported volume, bounded retries and backpressure. Check that limits and observability make failures, queue backlog and cleanup actionable without introducing unbounded work. Consider an established project mechanism or suitable library only when it addresses a demonstrated correctness or maintenance need; absence of a preferred abstraction or best practice is not itself a defect.
