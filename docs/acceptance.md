# Supported targets and verification

The initial binary target is macOS Apple Silicon (`aarch64-apple-darwin`).
Controlled checks run with Rust/Cargo 1.98.0 and local fake provider/host CLIs.
They cover configuration, provider protocols, durable outcomes, archive safety,
process cleanup, terminal navigation and read-only viewing. CI runs workspace
unit/integration tests, formatting, Clippy, helper/host checks and PTY scenarios.

The packaged executable is checked with disposable terminal sessions. Local
Homebrew checks cover installation, reinstallation, upgrade, rollback, saved
viewing, checksums and removal. Public-download installation is verified after
publishing each exact archive.

Older macOS releases, Intel macOS, Linux, Windows, other physical terminal hosts
and clean-machine installation need separate qualification. Fake-provider tests
do not establish real account/model access or compatibility with every provider
CLI version. See [provider protocols](executor-protocols.md).
