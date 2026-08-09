# aimeter

Your Claude Code rate limits, in the statusline. One binary, ~2 ms, no daemon.

![The segment annotated: model, reasoning effort and context window on the left; the 5-hour, weekly and model-scoped limits with how much is spent and when each resets on the right](segment.svg)

## Get it

**Download a binary** — [latest release](https://github.com/MarioPayan/aimeter/releases/latest):

| Platform | File |
|---|---|
| Linux, Intel/AMD | [`aimeter-x86_64-unknown-linux-gnu.tar.gz`](https://github.com/MarioPayan/aimeter/releases/latest/download/aimeter-x86_64-unknown-linux-gnu.tar.gz) |
| macOS, Apple silicon | [`aimeter-aarch64-apple-darwin.tar.gz`](https://github.com/MarioPayan/aimeter/releases/latest/download/aimeter-aarch64-apple-darwin.tar.gz) |
| macOS, Intel | [`aimeter-x86_64-apple-darwin.tar.gz`](https://github.com/MarioPayan/aimeter/releases/latest/download/aimeter-x86_64-apple-darwin.tar.gz) |

`tar -xzf` it and put `aimeter` anywhere on your `PATH`.

**Or ask Claude Code:**

> Install aimeter from github.com/MarioPayan/aimeter and wire it into my statusline

**Or one command:**

```bash
curl -fsSL https://raw.githubusercontent.com/MarioPayan/aimeter/main/install.sh | sh
```

That picks the right binary, drops it in `~/.local/bin`, and prints the snippet below.
It is thirty lines — [read it first](install.sh) if you would rather not pipe a stranger's
script into a shell. With Rust already installed, `cargo install --git
https://github.com/MarioPayan/aimeter` works too.

## Wire it up

Claude Code runs exactly one `statusLine` command, so compose it with whatever else you
print. (This is not a Claude Code plugin and cannot be one — a plugin can declare skills,
agents, hooks and MCP servers, but not a statusline.)

```bash
# ~/.claude/statusline.sh
printf '%s' "$(aimeter line)"
```

```json
// ~/.claude/settings.json
"statusLine": { "type": "command", "command": "bash ~/.claude/statusline.sh" }
```

`aimeter line --bar` adds a fill block beside each percentage. `NO_COLOR` drops the
escapes — the `—`, the `?` and the countdowns are text precisely so they survive it.

## Where the numbers come from

| Source | Gives | Needs |
|---|---|---|
| the statusline payload on stdin | model, effort, context window, `S` and `W` | nothing |
| `GET /api/oauth/usage` | every window, including the model-scoped one | your OAuth token |
| `~/.claude.json` | the same, as of whenever Claude Code last refreshed it | nothing |

> **The usage endpoint is undocumented, and reaching it means reading your token.**
> `aimeter fetch` reads the access token from `~/.claude/.credentials.json` and sends it
> to an endpoint Anthropic publishes for its own client, not for third parties. It works
> today and may stop without notice.
>
> The token is **read, never written** — aimeter does not refresh it, does not touch the
> credentials file, never logs it, and never stores it. On a 401 it falls back to the
> cache and lets Claude Code sort its own token out. Only the `limits[]` array from the
> response is written to disk.
>
> **`AIMETER_NO_FETCH=1` turns it off entirely.** Everything still works — stdin covers
> `S` and `W`, the cache covers the rest, and only the model-scoped limit gets less
> current. A long `AIMETER_REFRESH_SECS` is *not* an off switch: with nothing cached, the
> first run fetches regardless.

That cache is a cache. It was once observed 15.5 hours stale, reporting 78% for a weekly
window that had reset six hours earlier while the true figure was 0% — which is why the
endpoint exists here at all, and why every number carries its own age.

## Details worth knowing

**Staleness is per limit, not per segment.** stdin covers `S` and `W` and nothing else,
so when it arrives beside an old cache the model-scoped number stays grey and the `?`
stays with it.

**A window that has reset shows `—`,** not its last reading — that number describes a
counter which has gone back to zero.

**A window that reports no reset time gets no clock.** The two weekly windows do reset
together to the millisecond, so borrowing one for the other would almost certainly be
right. Almost is not a claim the API made.

**It never fails loudly.** Missing file, moved schema, bad JSON: prints nothing, exits 0.
A statusline that reports its own errors corrupts the console it is drawn in. The network
refresh runs in a detached child, so the segment never waits on a TLS handshake.

## Not here on purpose

- **Cost in dollars.** On a Max subscription it is `0` and marginal cost is zero;
  percent-of-limit is the real currency. The endpoint returns a `spend` object and it is
  dropped rather than written to disk.
- **A daemon, a watcher, a database.** One process, gone two milliseconds later.
- **Token history.** It tallied every transcript into a rollup for a while, to drive a
  TUI. The TUI went and the tally went with it; both are in the git history.

## Working on it

Rust is pinned in `.tool-versions`. CI runs these four on Linux and macOS.

```bash
cargo build --release && cargo test
cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

`segment.svg` is generated — run `python3 tools/segment-svg.py` after changing the
segment's shape or palette so the diagram cannot drift from what the binary prints.

MIT — see [LICENSE](LICENSE).
