# Architecture and ownership

The core crate resolves layered configuration, compiles profiles, freezes plans,
coordinates find/synthesis/verify and records results. It has no terminal host
handle. Executor adapters translate a frozen Request to a child process, decode
protocol observations and preserve raw evidence. The CLI owns command dispatch,
signals and durable worker lifetime. The TUI receives bounded typed events and a
final saved Report; rendering cannot dispatch execution.

Configuration discovery uses caller cwd, while --workdir controls review access.
A frozen plan keeps selected raw prompt/profile bytes, runner model/effort and
input paths. Later file or configuration changes cannot alter an active plan.
Placeholder insertion is one pass; inserted content and appended history are
literal. Empty/missing optional observations remain distinguishable.

Finders own source identities. Synthesis can consolidate only known input IDs;
verification retains original identity/location/source ownership and applies
qualified assessment changes to clones. Categories cannot bypass an explicitly
selected completeness gate. Failed attempts retain partial evidence without
becoming completed sources. Requested models are intent, not observed telemetry.

Shared admission reserves one call before each process attempt across concurrent
stages/retries. Waiting for a slot consumes no call. Whole-run deadlines include
queues/backoff and cancel owned execution with bounded cleanup. Per-stage limits
cannot raise an explicit global ceiling. Reopening observation never resets these
limits or reruns a provider. Call/time budgets are not dollar or token caps.

Archive storage uses directory descriptors, contained traversal and exclusive
writes/claim locks. Completion is durable only after all required artifacts are
saved. Reads do not repair, scaffold or resolve today's profiles/accounts. Saved
roster/stage settings win; supported old missing fields have explicit defaults,
while corrupt types and incomplete identities produce errors. Cleanup holds
observed claims and checks entry identity; it is not an atomic transaction against
all possible starts after inventory.

Ratatui/Crossterm own terminal modes and widgets, pulldown-cmark parses Markdown.
Untrusted text becomes inert display content. Terminal loss cancels and joins
execution; an observer exit and successful review are separate outcomes. Hosts
only carry the selected executable/argv and observe owned result artifacts.
See [configuration](configuration.md), [archives](archives.md),
[hosts](hosts.md) and [requirements](requirements/standalone-review-runtime/spec.md).
