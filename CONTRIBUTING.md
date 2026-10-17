# Development

Use Rust/Cargo 1.98.0 (also selected explicitly by CI), Git, native linker tools and Python
3.9+. An offline build requires the locked dependencies already present in the
local Cargo cache. This candidate is checked on macOS arm64; do not infer Linux
or native desktop support from dependency portability.

```sh
cargo fmt --all -- --check
cargo test --workspace --offline --locked -- --test-threads=1
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
cargo build --workspace --all-targets --offline --locked
python3 -B tests/compat/helper_observation_checks.py
python3 -B tests/compat/host_launcher_checks.py
python3 -B tests/compat/public_tui_checks.py
python3 -B tests/compat/input_disconnect_checks.py
```

Run each terminal-specific *_launcher_checks.py too. The [test guide](tests/compat/README.md)
lists their scope. PTY tests require an environment that permits owned controlling
terminals. They never operate an installed terminal application. Provider CLI
loopback checks are explicit opt-ins; do not run them as a substitute for a review
or to inspect accounts. Ordinary tests need no provider installation.

New tests should express a behavior contract using authored synthetic inputs,
observable results and ownership limits. Do not copy private prompts, archives or credentials into fixtures. Do not regenerate snapshots merely to pass.
Review [architecture](docs/design.md) and [security](SECURITY.md). Keep changes scoped and record the checks performed.

For bug reports, include the version/package revision, OS, terminal host and a
small sanitized reproduction. Keep private archives and credentials out of issues.
Changes should explain the user-visible behavior and include relevant verification;
new tests should assert observable contracts rather than mirror implementation.

The [release procedure](docs/releases.md) covers packaging, supported targets,
Homebrew qualification and publication. Do not publish a release directly from
an unqualified local build or infer native-host support from fake-host tests.
