# CLI workflows

Use `revust --help` for the maintained flag inventory. --help writes to stderr;
headless reports use stdout. config --runtime-only reads settings/paths without
prompts; config lists the ordinary catalogue/tasks; config --effective adds
extension policy and selected structured profile. All three are read-only.
The reference_revision JSON key is retained for consumers; its current value
revust-contract-v1 identifies this contract, not a historical source revision.

`init` populates caller .revust with resolved prompts and a settings template.
`--dump-defaults DIRECTORY` exports embedded files. Existing authored files are
preserved; malformed or partly configured files are not silently replaced.
`new --task NAME --run NAME` creates the task/round scaffold, preserving existing
scope/goal/profile/context. Task and round names are contained relative identities.

A round's input/ directory contains scope.md and optional goal.md, profile.md
and context/. The scaffold creates input/; author scope.md there before dry-run. Describe
the review subject/diff and boundaries explicitly in scope.md. `dry-run` freezes
and displays selected requests and deferred stage templates without execution.
It is not a reservation of future paid calls. Run a review only after authority
for its scope, profile, model/effort and budget is established:

```sh
revust --task example --run 01 --profile focused --no-tui
```

Useful flags include --workdir, --config-dir, --tasks-dir, --lenses,
--no-synthesis, --no-verify, --min-confidence, --markdown and --auto-exit.
JSON report finding presence gives exit 1, no active finding gives 0; execution,
configuration and persistence errors can return 2. Launch helpers use 3 for
launch/protocol failure; observation interruption may return 130. Exit status
alone does not establish complete usage or acceptance: inspect durable outcomes.
Failed, stopped and policy-failed runs save an explicit `outcome` in both the
report and final manifest. Execution exits 2 and can leave stdout empty; inspect
the saved archive for JSON/Markdown evidence. Incomplete terminal results honor
`--auto-exit` while displaying incomplete status; zero waits and any key cancels
the countdown. A persistence error never announces durable completion.

`view --archive DIRECTORY` opens saved evidence without running any provider or
loading today's profiles. --no-tui returns JSON, with --markdown for prose. View
success does not reuse the original review's finding exit code. Quitting the TUI
with `q` restores the terminal without printing a second report to terminal stdout.
Redirected stdout still receives JSON (or Markdown); `--no-tui` always prints the
saved result. An inspectable failed/stopped/policy-failed archive exits 2. This is
distinct from a corrupt/active archive refused without a usable result. Qualified
older empty markers can be reconstructed in memory from matching terminal
evidence; saved viewing never fills or repairs them. `stats` and the
corpus helper read saved observations; `cleanup --task NAME` is an explicit owned
archive deletion operation, never a prerequisite for viewing. See [archives](archives.md).

In the TUI use tab/shift-tab and pane shortcuts shown in the footer, arrow/hjkl
scrolling, page/home/end navigation, `/` to edit a findings filter, Enter to accept
and Escape to leave editing. Unicode input is preserved. Any key cancels a pending
auto-exit countdown. Cancel execution and close observation are distinct actions;
only durable completion makes a saved result complete. Exact screen bytes are
not a stable interface.

Host-side inspection helpers use their own small argument contract:

```sh
REVUST_BIN=/absolute/path/to/revust scripts/preflight.sh focused
REVUST_BIN=/absolute/path/to/revust scripts/task-state.sh example
REVUST_BIN=/absolute/path/to/revust python3 scripts/analyze-corpus.py --json
```

preflight.sh accepts an optional profile and a boolean --lenses readiness mode;
it does not accept review CLI flags. task-state.sh requires one task ID. Both
inspect the selected executable's effective configuration without provider calls.
