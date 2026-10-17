#!/usr/bin/env bash
# Inspect the selected task through the same read-only CLI configuration.
set -eu
script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd) || exit 1
if ! command -v python3 >/dev/null 2>&1; then
    echo 'error: Python 3 is required' >&2
    exit 1
fi
exec python3 "$script_dir/inspect-config.py" task "$@"
