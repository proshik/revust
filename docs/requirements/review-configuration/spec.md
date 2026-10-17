# Configuration requirements

- Default discovery uses .revust, ~/.config/revust and REVUST_* without aliases or
  automatic migration. Explicit roots accept arbitrary names. Caller cwd selects
  layers independently of review workdir; equivalent directory roots collapse.
- INI resolves per field, prompt files per whole winning document. Profiles keep
  selected executor/model/effort, stage inheritance and ordered lens composition.
  Unknown/invalid active metadata fails before dispatch. Explicit false/zero stays.
- Placeholder insertion is literal and one pass; raw winning source observations
  are archived independently from composed bodies. Frozen plans ignore later edits.
- Adjacent version-1 extensions preserve per-layer precedence, explicit null limit
  removal, array replacement and redacted effective origins. Structured profiles
  require selection and cannot shadow ordinary profile IDs.
- Inspection/validation/dry-run are read-only and call no providers. Init/export
  preserve authored content, including malformed/partial configuration. Writes
  validate and compare existing identity/bytes before replacement.

See [configuration](../../configuration.md), [extensions](../../extensions-v1.md)
and authored Rust interaction tests for executable examples. Historical captured
outputs are not current acceptance oracles.
