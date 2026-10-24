# Changelog

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
