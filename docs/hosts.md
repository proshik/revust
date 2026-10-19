# Host transactions

`scripts/launch-revust.sh` chooses one supported adapter using actual host context
and explicit REVUST_* controls. REVUST_BIN selects a literal executable; otherwise
PATH supplies revust. Caller cwd and selected configuration roots are forwarded
separately from shell-quoted fixed worker paths. User arguments live in a private
JSON transaction, not executable shell text. REVUST_AUTO_EXIT=0 suppresses an
injected default countdown; explicit --auto-exit is preserved.

Supported adapter paths: Guild, agterm overlays, tmux popup/window (including
agentdeck-window.sh), zellij, kitty, wezterm/kaku, cmux, herdr, Ghostty, iTerm,
Terminal.app and Emacs vterm. Backend existence is not physical-host acceptance.
Capability checks precede dispatch where detectable. Unknown acknowledgement
retains owned request/start/result artifacts and does not switch host or dispatch
another review. Repeated worker delivery uses an exclusive claim. Inspect retained
evidence before retrying manually. Host diagnostics are separate from report data.
Cleanup uses acknowledged owned IDs only, never whichever pane is now active.

Guild must advertise terminal protocol version 1 with start/status/wait/close and
archive_viewer. The request field archive_viewer contains absolute directory and
program. Open run uses literal `PROGRAM view --archive DIRECTORY`, never original
review argv/environment. Ambiguous start retries the same request key/descriptor;
wait receipts cannot switch job identity. Missing capability refuses before review.


Fake-host tests exercise protocol, arguments, loss/retention and result ownership.
They do not interpret native AppleScript in a real app or prove focus, geometry,
restart/reopening or permissions on a user's desktop.

### Proxy environment

All host launch transactions forward `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`,
`NO_PROXY` and their lowercase equivalents independently. Missing caller keys
are omitted; empty values are forwarded unchanged. Existing host environment
values are not cleared when the caller omits a key. Values travel in private
0700 transaction directories and 0600 descriptors, never shell arguments.
Launcher host diagnostics mask caller proxy values.

Codex and Claude inherit these settings. Isolated OpenCode preserves only these
eight inherited transport settings in addition to its isolated environment.
Explicit `executors.<name>.env` settings take precedence. This forwarding does
not decide which case an HTTP client prefers or how it interprets `NO_PROXY`,
empty values, proxy authentication or bypass rules; those remain client behavior.
Provider output is still archived verbatim: configured executors must not print
credentials. Real provider network compatibility requires separate qualification.
