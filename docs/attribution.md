# Third-party attribution

The product is MIT licensed. Applicable third-party notices are retained in
`third_party/` and included in complete packages.

- Fractional duration arithmetic follows Go 1.26.4 under the BSD-3-Clause
  notice and patent terms in `third_party/go/LICENSE` and `PATENTS`.
- Process supervision adapts Guild Rust process code under its declared MIT
  terms. Source identity and terms are retained in `third_party/guild/NOTICE`
  and `LICENSE`.
- Vendored Crossterm 0.29.0 retains its MIT notices. The local terminal input-loss
  patch is `third_party/crossterm/revust-pty.patch`.
- Cargo dependencies retain their declared licences. Native packages include
  available licence/notice files and an index under `third_party/cargo/`.
- Native packages include Rust library copyright and MIT/Apache texts under
  `third_party/rust/`. `Cargo.lock` records exact dependency versions.

Ratatui, pulldown-cmark, Serde, Chrono, nix and Unicode libraries are linked
through the locked Cargo dependency graph. The runtime requires no Go runtime
or desktop UI framework. Third-party notices must accompany redistribution.
