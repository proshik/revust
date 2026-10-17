#!/usr/bin/env bash
# Explicit window-mode entry point, sharing the single host transaction driver.
set -eu
export REVUST_TMUX_WINDOW=1
script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd) || exit 3
exec "$script_dir/launch-revust.sh" "$@"
