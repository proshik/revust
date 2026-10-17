# Controlled integration checks

Normal tests use synthetic inputs and local fake provider/host CLIs. Rust tests
cover configuration, composition, schema, stage ownership, budgets, archive
containment, read-only reconstruction, process cleanup and widgets.

After the workspace all-target build, run each *_launcher_checks.py and
helper_observation_checks.py with Python 3.9+. They install fake CLIs only inside
owned temporary roots. host_peer.py is a file-backed synthetic Guild/agterm peer;
other peers validate terminal-specific argument and ownership boundaries. Cases
include absent capabilities, malformed receipts, exact selected program/argv/cwd,
ambiguous acknowledgements, same-key retry, duplicate delivery, honest results,
owned cleanup and metadata-only helpers. They never control active applications.

public_tui_checks.py uses local providers and disposable controlling PTYs for 22
review/archive/filter/resize/signal/input-loss/countdown scenarios. Failed, stopped
and policy-failed runs each cover timed closing, zero delay and any-key retention;
quitting saved views with terminal stdout suppresses duplicate JSON/Markdown,
redirected output is preserved, and saved incomplete views retain exit 2 and leave archive bytes unchanged. Run
terminal_session_checks.py with the built terminal_probe example for direct mode
restoration/failure/navigation coverage, and input_disconnect_checks.py for the
bounded EOF/CPU probe. PTY access may require a suitable local execution context.

claude_cli_checks.py, codex_cli_checks.py, opencode_cli_checks.py and
opencode_checks.py are optional installed-CLI loopback harnesses with explicit
binary pins in docs/executor-protocols.json. They are not part of routine source
qualification and have not been run for the rewritten candidate. Never infer
account/model access, credential probes or a new budget from their existence.
All public fixtures are synthetic or separately identified provider protocol data;
no personal profiles, secrets or review archives are test inputs.

`distribution_checks.py` exercises the distribution validator with disposable Git
histories and archives. It covers historical metadata, unapproved files even
with recomputed checksums, dependency notice retention, source/receipt identity,
local build paths, unsafe entries and portable ownership. It uses no user data,
provider CLI or Cargo compilation. Release preparation additionally validates the
real built archives before upload.
