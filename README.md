# aimeter

Your Claude Code rate limits, in the statusline. One binary, ~2 ms, no daemon.

![The segment annotated: model, reasoning effort and context window on the left; the 5-hour, weekly and model-scoped limits with how much is spent and when each resets on the right](segment.svg)

## Why

**You never ask.** The number is already on screen, next to everything else you look at
a hundred times an hour. Asking Claude how much you have left costs a round trip, some
context, and the answer is a guess anyway.

**It spends nothing.** No model call, no tokens, no context. It reads files Claude Code
already writes and prints a line — checking your usage cannot itself be a reason your
usage went up.

**Three ceilings and a clock.** Session, this week, and the window scoped to whichever
model is capped — each with how much is gone and when it comes back. Plus how full the
context window is, which is the one you actually hit hourly.

**It never lies to you.** A number older than six hours goes grey. A window that has
already reset shows nothing rather than its last reading. A reset time the API did not
send is left blank instead of guessed.

**Nothing is running.** No daemon, no watcher, no database, no background process. One
short-lived process per render, gone in about two milliseconds — for comparison, anything
that spawns `node` costs 60–100.

## Get it

```bash
curl -fsSL https://raw.githubusercontent.com/MarioPayan/aimeter/main/install.sh | sh
```

Downloads the right binary, wires your statusline, and is safe to run twice — it backs up
an existing statusline script before appending, and never overrides a `statusLine` you
already set. Pass `--no-wire` to skip the wiring. Short enough to
[read first](install.sh), which you should do with anything you pipe into a shell.

**Or ask Claude Code:**

> Install aimeter from github.com/MarioPayan/aimeter

**Or take a binary** from the [latest release](https://github.com/MarioPayan/aimeter/releases/latest):

| Platform | File |
|---|---|
| Linux, Intel/AMD | [`x86_64-unknown-linux-gnu`](https://github.com/MarioPayan/aimeter/releases/latest/download/aimeter-x86_64-unknown-linux-gnu.tar.gz) |
| macOS, Apple silicon | [`aarch64-apple-darwin`](https://github.com/MarioPayan/aimeter/releases/latest/download/aimeter-aarch64-apple-darwin.tar.gz) |
| macOS, Intel | [`x86_64-apple-darwin`](https://github.com/MarioPayan/aimeter/releases/latest/download/aimeter-x86_64-apple-darwin.tar.gz) |

Anywhere else: `cargo install --git https://github.com/MarioPayan/aimeter`.

---

The model-scoped limit needs one network call, which means reading the OAuth token Claude
Code already stores. It is never written or stored, and `AIMETER_NO_FETCH=1` turns it off
— **[how it works](docs/how-it-works.md)** covers that, where every number comes from,
and what the segment refuses to do.

MIT — see [LICENSE](LICENSE).
