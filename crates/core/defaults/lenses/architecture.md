---
description: Component boundaries
---
## Component boundaries

Examine ownership, dependency direction and API contracts. Look for changes that make one component depend on assumptions another component cannot enforce, especially across concurrent work or failure recovery.

Where state crosses components, establish who owns consistency, resources and recovery. For database-to-broker workflows, inspect existing guarantees before proposing an outbox or equivalent. If an outbox is used, business and outbox rows must share a transaction; examine publisher concurrency, claim ownership or SELECT FOR UPDATE SKIP LOCKED transaction lifetime, and publish-before-mark duplicates. Trace stable event IDs, idempotent consumers, ordering and partition keys to the actual contract; an outbox alone does not provide exactly-once effects. Check semantic API, event and configuration compatibility across rolling upgrades and rollback, including data, migration and backup/restore compatibility.
