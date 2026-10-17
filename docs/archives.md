# Saved evidence and inspection

Each task/round retains input/scope.md, optional input/goal.md, input/profile.md
and input/context/, resolved prompts,
events.jsonl, stage reports and findings.json/report.md. manifest.json records
identity, timestamps, roster, selected profile, prompt provenance and stage intent.
The revust/ sidecar holds distinct attempts, raw streams, requests, policy and
extra acceptance evidence. Failed attempts are never concatenated into a later
attempt's apparent successful output. Raw authored prompt/profile observations
remain separate from composed requests and original candidates from refinements.

Writers use descriptor-relative contained paths, exclusive files and advisory
claim locks. Completed evidence is not overwritten. A nonempty marker alone is
not sufficient for valid replay: identity, completion and required artifacts are
checked. Readers accept qualified old missing roster fields and default nullable
optional fields, but reject corrupt non-null types or missing completion.
Explicitly selected root aliases are supported; descendant resolution must stay
contained. Input snapshot capture uses bounded no-follow reads, not arbitrary links.

Normal terminal execution saves a final marker for complete, partial, failed and
stopped runs. New `findings.json` and `manifest.json` carry the same explicit
`outcome` (runtime state, policy failures and final attempt diagnostics). A durable
archive is not a successful review: failed/stopped outcomes remain exit 2 when
executed or viewed and retain their evidence. Terminal failed, stopped and
policy-failed reports show an incomplete status while honoring an explicitly
configured auto-exit countdown; any key cancels it. With zero delay they wait for
quit. Saved archive browsing defaults to zero delay. Persistence errors are
observation errors, not completion.

Read-only viewing also supports historical zero-byte markers only when the saved
JSON/Markdown report has valid completion times and a contained, complete terminal
journal proves a matching failed/stopped identity. Its metadata/outcome are restored
in memory; the marker and every saved artifact stay untouched. Missing frozen
metadata remains unknown. No current profile or provider configuration is loaded.
An active claim, truncated/nonterminal/corrupt journal, contradictory identity,
nonempty corrupt marker or a new explicit-outcome report with an empty marker is
rejected. Marker ownership is checked under a shared nonblocking lock; descendants
still use bounded contained reads. This compatibility path never resumes execution.

The viewer uses saved settings, observed identity and report data, never today's
provider accounts or reusable profiles. Reopening cannot rerun a review. History
and statistics omit private finding bodies; aggregate counts describe recorded
observations, not total work or causal quality. Missing telemetry is unknown, and
zero fields in older formats may mean zero or unavailable. Failed/partial attempt
usage is accounted separately where observed.

`stats [--task NAME]` lists observed stages and qualified skipped inputs.
`scripts/analyze-corpus.py` is read-only metadata analysis; `scripts/task-state.sh`
shows configured task state. Both honor the selected executable and caller cwd.
`cleanup --task NAME` refuses active/ambiguous locks and confines deletion to its
owned selected root. It may leave partial deletion on I/O failure and does not
serialize against every possible future review start. Never run it on user data
as a test. Synthetic temp roots are the test boundary.
