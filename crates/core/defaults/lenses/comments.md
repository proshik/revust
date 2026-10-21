---
description: Explanations in source
---
## Explanations in source

Compare comments and API documentation to the code they describe. Flag incorrect guarantees or obsolete explanations that could cause a caller or maintainer to use the code incorrectly.

Inspect claims about transaction atomicity, SQL isolation, broker confirms, acknowledgements, ordering and exactly-once effects against the actual driver and broker behavior. Check resource ownership, retry safety and compatibility explanations where callers rely on them. Report a misleading or missing explanation only with the concrete misuse it enables; do not require commentary that merely repeats clear code.
