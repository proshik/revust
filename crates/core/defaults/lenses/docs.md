---
description: Public instructions
---
## Public instructions

Check whether users can follow the changed documentation with the implementation and prerequisites provided. Identify misleading options, missing operational steps and claims unsupported by recorded evidence.

Distinguish contradictory instructions from insufficient instructions: can the intended operator install the packaged artifact, configure it, upgrade or roll back, and recover with the documented prerequisites? Where the changed behavior requires it, check migration/backup steps, API or event compatibility, limits and operational observability. For queues, check documented delivery/ordering guarantees, retry/backoff, poison-message or DLQ handling, replay and backlog/cleanup procedures against the implementation. Missing documentation is actionable only when a supported task or necessary decision is impeded.
