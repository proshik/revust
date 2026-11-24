# Changelog

## Unreleased

- OpenCode providers files convert a model-level `variants` entry into the
  model's `settings` (merging with variants that arrived through `options`)
  instead of leaving it in place, where OpenCode 2.x rejects the whole
  provider. A non-object model `settings` value is replaced so the variant
  definitions are never dropped silently.
- The Homebrew formula template installs only the files listed in the release
  archive's SHA256SUMS, so Homebrew's build-time HOME no longer lands in
  `libexec` and repeated installs record identical sums.

## 0.1.9 — 2026-11-20

- Keep verification group prompt names distinct on case-insensitive volumes.
- `config --effective`, dry-run and review reject a required reviewer missing
  from the roster before claiming a round; finder names are unique without
  regard to case at run time too.
- Create no further verification worker threads after the run stops, and order
  retained findings by an ID index instead of a quadratic search.
- Launchers no longer write bytecode for the shared helper into the installation.
- OpenCode lookup finds nothing through an empty or unset PATH; preflight
  reports only regular executable files as available.
- Codex and OpenCode answers that repeat a JSON key are invalid output.
- The task inventory reads each task through one directory handle and reports
  non-UTF-8 round names instead of renaming them; a `task.md` link escaping its
  task directory is reported as a metadata error.
- The Homebrew formula template replaces the package when installing over a
  leftover keg instead of nesting it and binding helper interpreters twice.
- On macOS a review run keeps the Mac from idle sleep through
  `/usr/bin/caffeinate -i -w PID` until the run ends; policy
  `runtime.keep_awake: false` disables it, and a missing helper is one warning.

## 0.1.8 — 2026-11-19

- Keep every synthesis candidate: inputs synthesis leaves unclaimed stay in
  findings as reported and are verified when verification is enabled. A null or
  mistyped stage result array, and a verifier answer that assesses one ID twice,
  are now invalid output instead of an empty or order-dependent result.
- Freeze round inputs (scope, goal, profile, context) once before dispatch under
  `prompts/input/`, so every finder, stage and saved view reads the same bytes.
  Inputs and prior-round history are read with bounded, contained, nonblocking
  reads; absolute or escaping links and FIFOs in inputs are refused at planning.
- `init` no longer replaces a project config that holds any text besides blank
  lines, comments and section headers.
- Warn on stderr, with file and line but never the line text, about config text
  the INI reader ignores and bare carriage returns; parsed values are unchanged.
- Reject rosters whose finder names differ only by letter case while loading
  profiles, before a review claims its round.
- OpenCode lookup skips non-executable files and directories on PATH, selecting
  the same executable as preflight instead of failing the version probe.
- Run verification groups on a worker pool bounded by verify and global
  parallelism and the remaining call budget instead of one thread per group; a
  refused thread no longer aborts the run before its final report.
- Reject finder names `synthesis` and `verify …` (any case), which collided with
  stage rows in the TUI and retry statistics; rename such reviewers in custom
  profiles.
- Show saved diagnostics and Markdown written to a terminal as inert text;
  redirected output and JSON keep the saved bytes.
- Expire an unterminated bracketed paste after one second (at most 1 MiB) so
  Ctrl-C and `q` reach the TUI again.
- Count a supported task alias as its task in `task-state.sh` and `stats`
  totals; read-only helpers no longer write Python bytecode; corpus analysis
  reads snapshots up to 256 MiB and compares verification with finder output
  only when synthesis did not run.
- The Homebrew formula template checks the formula's resolved version; installed
  CLI cancellation checks expect the saved stopped outcome; extension edit
  documentation describes its byte snapshot and advisory lock scope.

## 0.1.7 — 2026-11-19

- Run built-in Codex agents on `gpt-6.2-sol`, and the expert profile's Codex
  agent on `gpt-6.2-astra`; effort and colors are unchanged.
- Stop Claude reviewers from running hooks defined in the reviewed checkout's
  project settings, which may come from a foreign branch.
- Accept `--tasks-dir`/`--config-dir` in `task-state.sh` and `--config-dir` in
  `preflight.sh`, forwarding them to `revust config`; `task-state.sh` reports
  `archive_in_worktree` when the archive would be deleted with a linked worktree.

## 0.1.6 — 2026-11-04

- Prioritize review investigation by changed behavior risks while keeping checks
  relevant to the assigned scope.
- Make security data-flow tracing, object/action authorization, tenant scope and
  sensitive-data exposure checks explicit in the adversarial lens.
- Inspect changed dependencies, CI and release paths for execution, permissions,
  secret access and artifact provenance.
- Reduce repetitive messaging examples across lenses while retaining transaction,
  delivery, idempotency, recovery, ordering and compatibility safeguards.

## 0.1.5 — 2026-11-04

- Prevent retry evidence filenames from colliding with supported finder names.
- Bound verification group filenames while preserving full directory keys.
- Align archive writer and reader size limits so large findings and aggregate
  event streams remain readable; reject oversized evidence before completion.
- Preserve frozen raw prompt documents, provenance and hashes separately from
  composed requests.
- Record terminal synthesis failure and cancellation states in live and saved
  views, including passthrough after retries.

## 0.1.4 — 2026-10-24

- Expand conditional review directions for transaction and messaging guarantees,
  compatibility, recovery, resource limits, documentation and regression coverage.
- Require causal contract evidence in built-in profile bodies and verification;
  document whole-document override limits for customized profiles and lenses.

## 0.1.3 — 2026-10-19

- Forward caller proxy settings through host launch transactions and isolated
  OpenCode executors, preserving case, empty values and explicit executor overrides.

## 0.1.2 — 2026-10-19

- OpenCode isolated sessions can load providers from a configured JSON/JSONC file
  (`executors.opencode.options.providers_file`), converting an opencode.jsonc
  document to native provider entries.

## 0.1.1 — 2026-10-17

- Update serde_json to 1.0.154, sha2 to 0.11.1, and serde-saphyr/granit-parser to 2.0.0.
- Validate full source history and distribution archives during CI and packaging.

## 0.1.0 — 2026-10-17

- OpenCode 2.x version admission and separate protocol compatibility diagnostics.
- Explicit durable execution outcomes, including failed/stopped archive viewing
  and read-only recovery of qualified historical empty markers.
- Quitting saved terminal views no longer prints a duplicate report to the screen;
  redirected and headless output remain available.
- Incomplete-result auto-exit with truthful status and any-key cancellation.
- Structured find, synthesis and verification with Claude Code, Codex and OpenCode.
- Reusable Markdown profiles/lenses, per-field INI layers and versioned JSON policy.
- Immutable input/prompt evidence, separate attempts and bounded cancellation.
- Read-only saved views, history, corpus statistics and contained cleanup.
- Native Ratatui navigation/filtering and plain progress from typed engine events.
- Owned host transactions and mandatory neutral Guild archive metadata.
- Complete macOS arm64 distribution with terminal helpers and Homebrew support.
- Public installation, rollback, contribution and release guidance.

See [supported targets and verification](docs/acceptance.md) for qualification limits.
