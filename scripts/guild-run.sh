#!/usr/bin/env bash
# Positional adapter for a host-owned job; the selected executable stays literal.
set -eu
if [ "$#" -lt 3 ]; then
    echo 'error: expected report, diagnostics and executable paths' >&2
    exit 3
fi
script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd) || exit 3
if ! command -v python3 >/dev/null 2>&1; then
    echo 'error: Python 3 is required' >&2
    exit 3
fi
exec python3 "$script_dir/launch-worker.py" --direct "$@"
