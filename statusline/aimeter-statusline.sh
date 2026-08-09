#!/usr/bin/env bash
# The aimeter statusline segment.
#
# Deliberately does almost nothing: find the binary, run it, and get out of the
# way. Everything that could be slow or could fail lives in `aimeter line`, which
# prints nothing and exits 0 on any error — a statusline that writes an error
# message corrupts the console it is drawn in.
#
# No arg parsing, no config, no caching. The binary is ~2ms; a wrapper that tried
# to memoise it would cost more than it saved.

bin="${AIMETER_BIN:-}"
if [ -z "$bin" ]; then
  # Relative to this script, so a clone anywhere works without editing a path.
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  for candidate in "$HOME/.local/bin/aimeter" "$here/../target/release/aimeter"; do
    [ -x "$candidate" ] && bin="$candidate" && break
  done
fi
[ -n "$bin" ] || exit 0

# Bounded where a bounding tool exists: this runs on every render, and a hung read
# on a network filesystem would otherwise stall the statusline. macOS ships no
# `timeout`, so there this runs unbounded — the binary's own work is ~2ms, and a
# wrapper that reimplemented the timeout would cost more than the risk it removes.
if command -v timeout >/dev/null 2>&1; then
  timeout 2s "$bin" line 2>/dev/null || true
else
  "$bin" line 2>/dev/null || true
fi
