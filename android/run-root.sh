#!/usr/bin/env bash
set -euo pipefail

remote_dir="${BINDER_TRACE_REMOTE_DIR:-/data/local/tmp/binder-trace}"
bin="${BINDER_TRACE_BIN:-binder-trace}"
remote_bin="$remote_dir/$bin"
device_id="${BINDER_TRACE_DEVICE_ID:-$(adb get-serialno 2>/dev/null || true)}"

remote_cmd="$(printf 'BINDER_TRACE_DEVICE_ID=%q ' "$device_id")$(printf '%q ' "$remote_bin" "$@")"
adb shell su -c "$remote_cmd"
