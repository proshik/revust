# Preparing a local native package

The packaging helper builds a complete artifact from one committed revision and
never installs or publishes. Use the pinned toolchain, local cached locked Cargo
sources, Git, tar and Rust licence documentation. Choose a new output directory:

```sh
python3 scripts/package-local.py --revision HEAD --output /tmp/revust-candidate
cd /tmp/revust-candidate
shasum -a 256 -c SHA256SUMS
```

The output includes revust-source.tar.gz, a target-named payload, build.log,
build.json and SHA256SUMS. The payload contains bin/revust, helpers, packaged
workflow skill, docs, Cargo.lock and product/dependency/Rust notices. build.json
pins revision, target, toolchain, source/binary/lockfile checksums. Unpack into an
owned temporary directory and verify the internal SHA256SUMS too.

Inspect expanded archive members, link targets, all uncompressed files and binary
strings for unwanted historical content. Verify helper/skill/docs links and every
applicable notice. A compressed-byte grep cannot verify source/payload closure.
Run config/view and controlled tests with the packaged executable in a synthetic
HOME; do not point them at personal profiles or archives. Preserve logs privately.

`integrations/homebrew/revust.rb.in` is the formula template. Fill its release
URL and checksum from the exact built archive, then verify installation and
saved viewing through Homebrew. Keep source, executable and checksum identities
consistent. Packages contain product files selected by the packaging manifest.
