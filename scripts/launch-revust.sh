#!/usr/bin/env bash
# Preserve argv while handing host protocol ownership to the standard-library driver.
set -eu
script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd) || exit 3
if ! command -v python3 >/dev/null 2>&1; then
    echo 'error: the host launcher requires Python 3' >&2
    exit 3
fi
exec python3 "$script_dir/host-launch.py" "$@"
