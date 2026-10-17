# Runtime requirements

- Ordinary terminal and headless execution require no desktop host. Providers are
  explicit selected child CLIs, independent of the initiating harness. Unsupported
  detectable capabilities fail before dispatch without silent model substitution.
- Find, synthesis and verify preserve source/category identity and original versus
  refined evidence. Failed/partial results cannot become complete sources. Optional
  strict acceptance checks all relevant categories; disabled stages stay disabled.
- Atomic shared admission counts attempts including retries, not waiting. Whole-run
  elapsed deadlines include backoff/queues, cancel owned descendants and preserve
  partial evidence. Reopening observation never resets a budget or starts execution.
- Save raw input/profile/prompt observations, per-attempt stdout/stderr/requests,
  provider observations and final/partial outcomes durably. Unknown usage/model
  remains unknown. A process exit or disconnected observer is not durable success.
- Descriptor-contained writes and reads reject escapes; advisory claims and
  exclusive artifact creation prevent overwriting completed evidence. Viewer,
  history and stats do not repair archives or load today's profiles/accounts.
- TUI preserves pane/tab navigation, filtering/editing Unicode and saved report
  immutability. Any key cancels auto-exit. Resize/input loss is bounded and joins
  owned execution without treating cancellation as successful completion.
- Host adapters keep literal argv/cwd/program identity and loss-aware owned
  acknowledgements. Unknown starts never trigger backend fallback or duplicate
  review. Guild requires archive_viewer and literal saved viewer arguments.
- Qualification records an exact source revision, target and package checksum.

See [archives](../../archives.md), [hosts](../../hosts.md) and
[acceptance limits](../../acceptance.md).
