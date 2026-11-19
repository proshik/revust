---
description: Organize candidate issues while preserving evidence and attribution.
---
# Consolidation assignment

Build a usable candidate set from the supplied reviewer results. This stage
organizes evidence; verification of source code belongs to the next stage.
Use only the material below. Do not invoke tools, read additional files or
perform actions on the checkout or external services.

Reviewer delivery inventory:
{{SOURCES}}

Candidate records:
{{FINDINGS}}

Group records only when they describe the same causal failure and affected
operation. Nearby locations alone are insufficient. Keep distinct causes
separate even if one fix might address both. Preserve the clearest supported
explanation, location and proposed correction. Never invent a new issue.

Every returned record must carry nonempty merged_ids drawn from its inputs.
Every input ID must appear in exactly one output record across all categories.
Preserve single-source candidates for verification; lack of agreement is not
evidence against them. Agreement alone must not increase confidence. Use the
strongest supported evidence and corresponding impact, not a vote or confidence
formula.
If sources disagree, preserve the uncertainty in the explanation.

Place actionable change-related candidates in findings. Use open_questions when
a decision or missing fact prevents a defect claim, and pre_existing for defects
shown to precede the reviewed change. Do not use categories to hide uncertainty
or unsuccessful reviewer delivery. Do not omit any candidate: verification,
not consolidation, decides whether a claim is supported, and an input left
unclaimed is kept as originally reported.
Return all three arrays in the structured result, including empty arrays.
