# Adjacent policy, version 1

Place extensions.json beside a layer's config. It is optional and read-only on
load. Use `revust config --extension-schema` for the exact strict JSON schema;
`config --effective` reports resolved policy and origins with environment values
redacted. Unsupported versions, unknown fields or invalid referenced profiles
fail before dispatch.

```json
{"version":1,"policy":{"budget":{"max_calls":4,"elapsed":"10m"},
 "stages":{"verify":{"enabled":false}}},"profiles":{}}
```

Profiles map IDs to a policy overlay, or a file under extension-profiles/NAME.md
plus policy. Structured profiles require explicit selection and cannot shadow an
ordinary profile. Relative references resolve from the declaring layer and stay
inside extension-profiles. Their frontmatter uses runner objects with executor,
model and effort, plus agents and stages. OpenCode requires explicit selection.

| Policy object | Fields |
| --- | --- |
| runtime | idle_timeout, hard_timeout, stagger_delay, max_parallel, verify_groups, verify_group_by, codex_sandbox |
| stages.find/synthesis/verify | idle_timeout, hard_timeout, parallelism; synthesis/verify also enabled |
| retries.find/synthesis/verify | count, conditions, delay: base, factor, max, jitter |
| verification | min_directory_group_size, min_confidence |
| budget | max_calls, elapsed |
| completeness | required_reviewers, min_successful_reviewers, require_synthesis_success, require_all_verified, synthesis_failure |
| executors.claude/codex/opencode | executable, env, options |
| presentation | host, mode, geometry, unavailable |
| output | tui, format, auto_exit |

Objects merge by key; arrays replace. False and zero are explicit values. Null
removes optional max_calls, elapsed and delay.max. Other limits must satisfy the
exported schema: positive parallelism/group counts, nonnegative retry counts,
confidence 0-100, finite delay factor at least one. Per-layer stage policy refines
that layer's global settings; a later global assignment can override it. Order:
defaults, user INI/extensions/profile policy, project INI/extensions/profile policy,
invocation. Explicit overall parallelism remains a ceiling.

Default find/synthesis permit one retry for failed delivery; verify permits none.
A delivered invalid answer is not retried unless invalid_output is explicitly
selected. Conditions may be legacy or an array of process_error, idle_timeout,
hard_timeout, rate_limit, nonzero_exit, missing_output, invalid_output. Empty means
no eligible errors. Default find delay is 5s plus sampled jitter below 5s;
synthesis delay is zero. Caps include jitter. Whole-run budget covers every attempt.

Completeness is opt-in. Required reviewer failures, missing synthesis success or
unverified candidates cannot satisfy a selected gate. synthesis_failure accepts
legacy, fail or passthrough; passthrough retains original candidates marked partial.
Executor paths are literal executable identities, not shell programs. Environment
overrides are child-only. Parsing presentation policy does not qualify a native
host. Validate actual provider support separately; see [protocols](executor-protocols.md).
Configuration edit API validates the entire proposed file and compares inode/bytes
before atomic replacement, preserving concurrent authored changes.
