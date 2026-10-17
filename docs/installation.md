# Installation, package selection and rollback

## Distribution status

Release [0.1.0](https://github.com/proshik/revust/releases/tag/v0.1.0) downloads
and the Homebrew formula are available for macOS Apple Silicon. Source builds use Rust/Cargo 1.98.0; native
packaging also needs Git, Python 3.9+, tar, cached locked dependencies and Rust
standard-library licence documentation. See [packaging](local-packaging.md).

A complete package keeps `bin/revust`, `scripts/`, `integrations/`, documentation,
`build.json`, `SHA256SUMS` and `third_party/` together. Verify the outer download
checksum and internal checksums before selecting it. Only run downloaded helpers
from a trusted, verified package.

## Homebrew installation

```sh
brew install proshik/tap/revust
revust --version
revust config --runtime-only
```

The formula retains the full package under its `libexec` directory and expose
the executable on PATH. Provider CLIs and their authentication are separate.
Installing revust must not modify user profiles, archives or agent settings.
Homebrew binds helper interpreters to its declared Python dependency. Original
package checksums are retained as `SOURCE_SHA256SUMS`; `SHA256SUMS` describes the
installed files after that binding. The binary and original build receipt stay
unchanged. Verify installed checksums from the package's `libexec` directory.

## Workflow skill and host launcher

The shared skill is `integrations/skills/revust/SKILL.md` inside the full package.
Its relative resources require that package layout; copying the skill file alone
is insufficient. Register it using the selected harness's supported mechanism.
Avoid duplicate registrations or hooks that start a second review.

For a native host, select the package's binary and run its launcher from the
calling project, retaining the real host session environment:

```sh
REVUST_BIN=/absolute/package/bin/revust \
  /absolute/package/scripts/launch-revust.sh --task example --run 01 --profile focused
```

This command executes providers. First create inputs and inspect `dry-run` as
described in [usage](usage.md). A compatible Guild exposes `archive_viewer`; do
not substitute another host silently when its capability check fails.

## Stable package selection

Existing owner-managed installations may use a `current` symlink selecting a
complete immutable package. Keep command, helper and skill registrations pointed
through the same selector. Record the previous target and receipt, verify the new
package, settle active review execution, then replace the selector atomically.
An already running saved viewer can keep using its retained previous package.

Selecting a verified Homebrew package can later use its stable `opt/revust/libexec`
path. Do not pin registrations to a versioned Cellar directory. A selector change
does not migrate configuration or archives and does not authorize another review.

## Rollback and removal

Keep the previous complete package and recorded selector target until acceptance
is finished. Settle active execution before restoring that target and verify the
resolved executable and skill resources. Preserve newer user-authored settings
and all saved observations; rollback does not rewrite them.

For Homebrew, qualify upgrade/removal with disposable configuration first. The
formula must not remove `.revust` or `~/.config/revust`, modify agent instructions,
or switch an owner-managed selector automatically. Restore or remove registrations
explicitly before removing a package they still reference.
