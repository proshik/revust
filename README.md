# revust

[![Release](https://img.shields.io/github/v/release/proshik/revust)](https://github.com/proshik/revust/releases/latest)
[![Homebrew](https://img.shields.io/badge/Homebrew-proshik%2Ftap%2Frevust-orange)](https://github.com/proshik/homebrew-tap/blob/master/Formula/revust.rb)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/proshik/revust/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/proshik/revust/actions/workflows/ci.yml)
[![Qualified target: macOS arm64](https://img.shields.io/badge/qualified-macOS_arm64-lightgrey.svg)](docs/acceptance.md)

revust coordinates code reviews through locally installed Claude Code, Codex and
OpenCode CLIs. It runs finding, synthesis and verification stages, saves the
inputs and outcomes, and lets you inspect results in a terminal or as JSON/Markdown.
Reusable profiles select the reviewers, models, effort and review lenses.

The Rust engine runs without a desktop application. Optional terminal-host
adapters present the same review; Guild can also reopen a saved run. Viewing an
archive never starts a provider or changes its recorded observations.
Failed and stopped reviews remain inspectable with their diagnostics. A saved
execution outcome distinguishes durable evidence from a successful review.

## Installation and status

Release [0.1.0](https://github.com/proshik/revust/releases/tag/v0.1.0) and the
Homebrew formula are available. The locally qualified target is **macOS Apple Silicon** (`aarch64-apple-darwin`). Intel macOS,
Linux, Windows and other native terminal hosts need separate qualification.

Install with Homebrew:

```sh
brew install proshik/tap/revust
```

Or build from source with Rust/Cargo 1.98.0, Git and native linker tools:

```sh
cargo fetch --locked
cargo build --release --offline --locked -p revust
./target/release/revust --version
```

The examples below assume `revust` is on PATH. A complete distribution also
includes host helpers, the shared workflow skill and dependency notices; retain
those files together. See [installation and rollback](docs/installation.md) and
[local packaging](docs/local-packaging.md).

## Quick start

From the project you want to review, inspect settings and prepare a round:

```sh
revust config --runtime-only
revust config --effective --profile focused
revust new --task example --run 01
# Create .revust/tasks/example/01/input/scope.md with the review subject and boundaries.
revust dry-run --task example --run 01 --profile focused
```

Configuration inspection, `new` and `dry-run` do not execute providers. Inspect
the selected models and requests before starting the review:

```sh
revust --task example --run 01 --profile focused
# JSON output without the TUI:
# revust --task example --run 02 --profile focused --no-tui
```

Use a new prepared round for another execution. The TUI supports pane navigation,
scrolling and `/` filtering. Reopen the saved round at any time:

```sh
revust view --archive "$PWD/.revust/tasks/example/01"
revust view --archive "$PWD/.revust/tasks/example/01" --no-tui
```

Review commands may incur provider charges. The selected provider CLI must be
installed and authenticated separately; revust does not include model access.
Call/time limits constrain execution but are not dollar or token spending caps.
Archives contain source context and provider output; keep private archives out
of issue reports. See [provider boundaries](docs/executor-protocols.md).
Incomplete execution exits 2 and may leave stdout empty; use saved viewing to
retrieve the report. Viewing an incomplete outcome also exits 2. Use `--no-tui` or redirect stdout
to export its evidence; quitting the TUI restores the terminal without printing
a duplicate report. Automatic closing of a terminal result does not turn failure into
success. See [exit semantics and countdown](docs/usage.md).

## Configuration and hosts

Settings layer from embedded defaults, `~/.config/revust`, project `.revust` and
invocation flags. Profiles and lenses are Markdown; optional execution policy is
versioned JSON. `--workdir` selects reviewed code separately from configuration
discovery. User-authored profiles and saved observations retain their own bytes.

Terminal-host helpers require Bash and Python 3.9+. Guild requires terminal
protocol version 1 with `archive_viewer`; hosts without it are refused before
execution. Host integration is optional for ordinary CLI/headless use.
The OpenCode adapter accepts parseable 2.x versions and reports unsupported
argument/event formats explicitly. Version admission is not qualification of
every 2.x release; see the recorded [provider boundaries](docs/executor-protocols.md).

## Documentation and development

Read [usage](docs/usage.md), [configuration](docs/configuration.md),
[extensions](docs/extensions-v1.md), [architecture](docs/design.md),
[host adapters](docs/hosts.md), [archives](docs/archives.md) and
[verification limits](docs/acceptance.md). [CONTRIBUTING](CONTRIBUTING.md) describes
controlled development checks; [SECURITY](SECURITY.md) describes sensitive reports.

The product is [MIT licensed](LICENSE). Retained dependencies have their own
[attribution and notices](docs/attribution.md), included in complete packages.
