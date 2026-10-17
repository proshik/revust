# Release procedure

Release [0.1.0](https://github.com/proshik/revust/releases/tag/v0.1.0) is published.
A release is accepted only for its exact
source revision, target, archive checksum and recorded checks. The initial target
is `aarch64-apple-darwin`; do not advertise other platforms from source portability.

## Candidate gates

1. Complete the checks in [CONTRIBUTING](../CONTRIBUTING.md) with controlled
   executors. Ordinary CI must not use provider credentials or real models.
2. Check the complete package from an exact committed snapshot, including notices,
   helpers, skill resources, executable permissions and dynamic library links.
3. Verify configuration, preparation, dry-run, saved viewing and packaged TUI in
   disposable HOME/configuration roots. Check archives remain byte-identical.
4. Check native host delivery separately and record physical Open run/restart
   coverage. Identify unrun real-provider and platform scenarios explicitly.
5. Check source files, Git metadata and expanded release assets for secrets,
   personal data and unintended files. Build receipts, source archives and tags
   must identify the exact published source revision.

## Build and publication

The manual `release-candidate.yml` workflow prepares artifacts without publishing
a tag, GitHub release or tap change. The local equivalent is
`scripts/package-local.py`; see [packaging](local-packaging.md).

Choose a version in workspace metadata and changelog, commit it, then build from
that exact commit. Retain the source archive, target archive, outer `SHA256SUMS`
and `build.json`. Verify internal checksums too. Record compiler identity and
support limits; checksums identify artifacts but do not authenticate a publisher.
Do not claim signatures, attestations or byte reproducibility without supplying
and verifying that evidence.

Record the final commit, tag, release notes and checksums. Require passing
hosted CI before publishing the tag and exact assets.
Never silently replace a published version's asset with different bytes.

## Homebrew

`integrations/homebrew/revust.rb.in` is a template, not an installable formula.
Prepare `Formula/revust.rb` in the distribution tap using the actual release URL and
SHA256. Audit interpreter/runtime dependency discovery with a minimal PATH;
do not assume the developer's Python installation exists on a clean machine.
Preserve the full package in `libexec` and expose the executable through `bin`.
Bind interpreter paths to the declared dependency during installation, retain
the original package sums and regenerate installed-file sums. Keep package
README/license/changelog in `libexec` as well as Homebrew's prefix metadata.

Before publication, run style/audit, a meaningful `brew test` and disposable
installation from the exact candidate archive. Exercise reinstall, upgrade,
rollback and removal while checking user-owned files stay intact. After
publication, repeat installation using the anonymous public download URL and
documented `brew install proshik/tap/revust` command.

Only then update release/Homebrew badges and installation status. Keep release
notes focused on behavior, supported targets, known limits and upgrade steps.
