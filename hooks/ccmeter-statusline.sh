#!/usr/bin/env bash
# The ccmeter statusline segment.
#
# Deliberately does almost nothing: find the binary, run it, and get out of the
# way. Everything that could be slow or could fail lives in `ccmeter line`, which
# prints nothing and exits 0 on any error — a statusline that writes an error
# message corrupts the console it is drawn in.
#
# ponytail: no arg parsing, no config, no caching. The binary is ~2ms; a wrapper
# that tried to memoise it would cost more than it saved.

bin="${CCMETER_BIN:-}"
if [ -z "$bin" ]; then
  for candidate in "$HOME/.local/bin/ccmeter" "$HOME/repos/Kaze/ccmeter/target/release/ccmeter"; do
    [ -x "$candidate" ] && bin="$candidate" && break
  done
fi
[ -n "$bin" ] || exit 0

# Bounded, because this runs on every render: a hung read on a network filesystem
# would otherwise stall the statusline indefinitely.
timeout 2s "$bin" line 2>/dev/null || true
