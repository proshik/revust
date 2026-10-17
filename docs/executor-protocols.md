# Provider boundaries

Adapters invoke local CLIs using argument arrays and stdin, preserving selected
model, effort, schema, cwd and explicit false/zero controls. Requested models are
never substituted from the initiating harness. Raw stdout/stderr are archived
before decoding; JSON transport observations are not stage-schema acceptance.
Incomplete/truncated/failed output is retained and qualified rather than promoted
to successful delivery. Unknown model/token observations remain unknown.

Claude uses JSONL assistant/result events, permission observations and structured
output; Codex uses a JSON answer plus diagnostic/model/footer observations and
selected-session rollout activity. Bounded record assembly handles arbitrary
chunks/UTF-8 boundaries. Rollout observations never select another session, reset
an ended deadline or dispatch another provider. Owned process groups, descendant
cleanup and fair nonblocking pumps enforce cancellation/idle/hard deadlines.

OpenCode uses isolated per-attempt configuration/HOME/XDG/TMPDIR and read-only
review agent policy. It accepts OpenCode 2.x (numeric major/minor/patch, including
prerelease/build suffixes) and requires an explicit provider/model. Version output
may be `opencode v2.minor.patch`, `v2.minor.patch` or `2.minor.patch`. Other majors,
unparseable output and failed/incomplete bounded version probes are rejected
before review dispatch. Accepting a version is not qualification of every 2.x CLI.
Generic effort is rejected, while model #variant is literal. Only qualified native
provider package options are accepted; implicit package installation and user-auth
import are not promised. Explicit provider overrides remain child-only. The
--preserve-anthropic-api-key opt-in affects a selected child; default inherited
key exclusions and OpenCode isolation are separate policies. No credentials are
printed by inspection.

An explicit rejection of adapter CLI arguments or a successful process without a
structured result due to undecodable JSONL/unsupported final-response events
reports what was not understood and that the revust OpenCode adapter may need
updating. Raw stdout, stderr and delivery diagnostics remain archived. Unknown
or malformed records alongside a supported result stay tolerated. Provider errors
retain their messages;
nonzero provider failures, timeouts and cancellation retain their classifications.
Understood model text without valid review JSON follows normal missing-output
or schema validation and does not establish a CLI incompatibility.

`executor-protocols.json` pins CLI identities used by optional loopback checks;
it records protocol inputs, not fresh acceptance of this candidate. The normal
suite uses fake executors only. Claude 2.1.296 and Codex 0.162.1 loopback harnesses,
and OpenCode 2.0.21, require explicit opt-in and their exact pinned binary. They
must not run through personal accounts to test a source change. No fresh live,
loopback installed-CLI or Linux qualification is claimed here. Tool permissions
are provider-specific; a parsed flag is not a universal sandbox guarantee.
