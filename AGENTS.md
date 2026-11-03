# Working on revust

revust is a Rust workspace that coordinates reviews through locally installed
Claude Code, Codex and OpenCode CLIs. Start with [CONTRIBUTING.md](CONTRIBUTING.md)
and [docs/design.md](docs/design.md); keep changes scoped and report checks run.

## Code map

- `crates/core`: layered configuration, prompt/profile resolution, frozen plans,
  find/synthesis/verify orchestration, reports and archive storage. Embedded
  profiles/lenses live in `crates/core/defaults`, output schemas in
  `crates/core/schemas`.
- `crates/executors`: provider request translation, protocol decoding, raw
  observations and child-process supervision.
- `crates/cli`: the `revust` binary, command dispatch, input snapshots, signals,
  execution lifetime and saved archive viewing.
- `crates/tui`: observation-only state, bounded event updates, terminal handling
  and rendering. Rendering must not dispatch reviews.
- `scripts` and `integrations`: launchers, host helpers, packaging and the shared
  review workflow skill. Host adapters are optional; the engine runs standalone.
- `crates/*/tests` and `tests/compat`: Rust contract tests and controlled Python
  integration checks. `third_party/crossterm` is a local Cargo patch for terminal
  loss and interrupted reads; preserve its qualified polling behavior.

## Checks

Use Rust/Cargo **1.98.0**, Git, native linker tools and Python **3.9+**. The
qualified target is macOS arm64; portability of dependencies is not evidence of
support for other targets. Run relevant checks for the change; the full CI
sequence is in [.github/workflows/ci.yml](.github/workflows/ci.yml):

```sh
python3 -B tests/compat/distribution_checks.py
python3 -B scripts/check-distribution.py
cargo fmt --all -- --check
cargo test --workspace --offline --locked -- --test-threads=1
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
cargo build --workspace --all-targets --offline --locked
```

Offline commands require cached locked dependencies (`cargo fetch --locked`
populates them). After the all-target build, relevant controlled checks include:

```sh
python3 -B tests/compat/proxy_forwarding_checks.py
python3 -B tests/compat/helper_observation_checks.py
for check in tests/compat/*_launcher_checks.py; do python3 -B "$check"; done
python3 -B tests/compat/public_tui_checks.py
python3 -B tests/compat/terminal_session_checks.py
python3 -B tests/compat/input_disconnect_checks.py
```

See [tests/compat/README.md](tests/compat/README.md) for scope and prerequisites.
Normal checks use synthetic inputs and fake providers/hosts; PTY checks need
owned controlling terminals. Installed-provider loopback checks are explicit
opt-ins, not routine verification. Do not infer authorization for model calls or
account inspection from a test harness. Test observable contracts, not copied
implementation; never use private prompts, archives or credentials as fixtures.

## Contracts to preserve

- Configuration discovery follows caller cwd; `--workdir` selects reviewed code
  independently. Freeze selected prompt/profile bytes, inputs, models and effort
  before execution. Placeholder expansion is one pass; inserted text is literal.
- Core has no terminal-host handle. Executors translate frozen requests and
  preserve raw evidence; hosts carry selected executable/argv and observe owned
  artifacts. Keep execution, observation and presentation separate.
- Finders own source identities. Synthesis references known IDs; verification
  preserves identity/location/source ownership. Failed attempts retain separate
  evidence and must not become completed sources.
- Reserve a shared call before each process attempt, including retries. Global
  deadlines include queues/backoff; cancellation cleans up owned execution.
  Call/time limits are not monetary or token caps.
- Archive writes are contained, exclusive and claim-locked. Saved viewing is
  read-only: never repair files, load current profiles/accounts or rerun providers.
  Durable evidence does not imply a successful review; incomplete execution and
  viewing retain exit code 2. Missing telemetry remains unknown.
- Treat provider text as inert display content. Preserve terminal restoration
  and bounded cleanup on input loss; never test cleanup against user archives.

## Documentation and distribution

Keep behavior changes aligned with [usage](docs/usage.md),
[configuration](docs/configuration.md), [extensions](docs/extensions-v1.md),
[archives](docs/archives.md), [host contracts](docs/hosts.md) and
[executor protocols](docs/executor-protocols.md). Baseline requirements live in
`docs/requirements`; qualification limits are in [acceptance](docs/acceptance.md).
Follow [SECURITY.md](SECURITY.md) for sensitive evidence and
[releases](docs/releases.md) for packaging and publication.

`distribution.json` is a reviewed source/payload allowlist, not generated output
to refresh from a dirty checkout. Validation requires full Git history and
GitHub noreply author/committer emails. Preserve third-party attribution.
Only the root `AGENTS.md` is allowed as a public source document, listed in
`source` but never `payload`. Other workflow files and nested `AGENTS.md` files
remain forbidden in distributed sources and packages.
