# How AImeter works

The short version is in the [README](../README.md). This is everything else.

## Installing by hand

Take the binary for your machine from the
[latest release](https://github.com/MarioPayan/AImeter/releases/latest), `tar -xzf` it,
and put `aimeter` anywhere on your `PATH`:

| Platform | File |
|---|---|
| Linux, Intel/AMD | [`aimeter-x86_64-unknown-linux-gnu.tar.gz`](https://github.com/MarioPayan/AImeter/releases/latest/download/aimeter-x86_64-unknown-linux-gnu.tar.gz) |
| macOS, Apple silicon | [`aimeter-aarch64-apple-darwin.tar.gz`](https://github.com/MarioPayan/AImeter/releases/latest/download/aimeter-aarch64-apple-darwin.tar.gz) |
| macOS, Intel | [`aimeter-x86_64-apple-darwin.tar.gz`](https://github.com/MarioPayan/AImeter/releases/latest/download/aimeter-x86_64-apple-darwin.tar.gz) |

Anywhere else — Linux on ARM, BSD, anything without a prebuilt binary:

```bash
cargo install --git https://github.com/MarioPayan/AImeter
```

Then point a statusline script at it. `exec` matters: it passes stdin through, and that
payload is where the model, the reasoning effort and the context window come from.

```bash
# ~/.claude/statusline.sh
exec aimeter line
```

```json
// ~/.claude/settings.json
"statusLine": { "type": "command", "command": "bash ~/.claude/statusline.sh" }
```

Already have a statusline script? Append to it instead — but check whether something
earlier in it consumes stdin, because that costs the whole left half of the segment:

```bash
printf '  %s' "$(aimeter line)"
```

### What the installer does, if you would rather it did it

`install.sh` is the same steps, and it is careful with the files it touches:
`AIMETER_DEST` chooses where the binary goes (default `~/.local/bin`); an existing
statusline script is copied to `statusline.sh.before-aimeter` before a line is appended;
`statusLine` in `settings.json` is only ever set when it is not already set, and every
other key is preserved; a second run changes nothing; and `--no-wire` installs the binary
alone and prints the snippet.

## Where the numbers come from

| Source | Gives | Needs |
|---|---|---|
| the statusline payload on stdin | model, reasoning effort, context window, `S` and `W` | nothing |
| `GET /api/oauth/usage` | every window, including the model-scoped one | your OAuth token |
| `~/.claude.json` → `cachedUsageUtilization` | the same object, as of whenever Claude Code last refreshed it | nothing |

Claude Code pipes a statusline command about fifteen fields on every render; four are
read. `context_window.used_percentage` and `effort.level` are free — no credentials, no
network — and they are the only two that describe a ceiling you are moving toward or the
reason you are moving toward it quickly. Cost is not read: on a Max subscription
`total_cost_usd` is `0`, and percent-of-limit is the real currency.

Ranking is by recency, so nothing needs a flag to say which source is in play. stdin wins
on the numbers it carries because it is regenerated every render. The model-scoped window
exists only in the other two.

### The token

> **The usage endpoint is undocumented, and reaching it means reading your token.**
> `aimeter fetch` reads the access token from `~/.claude/.credentials.json` and sends it
> as a bearer token to an endpoint Anthropic publishes for its own client, not for third
> parties. It works today and may stop without notice.
>
> The token is **read, never written.** AImeter does not refresh it, does not touch the
> credentials file, never logs it, and never stores it. On a 401 it falls back to the
> cache and lets Claude Code sort its own token out — refreshing it here could invalidate
> the one your editor is using. Only the `limits[]` array from the response is written to
> disk; the response also carries a `spend` object, which is dropped.
>
> **`AIMETER_NO_FETCH=1` turns it off entirely.** Everything still works: stdin covers
> `S` and `W`, the cache covers the rest, and only the model-scoped limit gets less
> current. A long `AIMETER_REFRESH_SECS` is *not* an off switch — with nothing cached,
> the first run fetches regardless.

### Why the endpoint exists here at all

`~/.claude.json` is a cache, and it is refreshed on Claude Code's own slow schedule.
`fetchedAtMs` sat unchanged through thirty minutes of continuous API traffic, and was
once observed **15.5 hours stale** — reporting 78% for a weekly window that had reset six
hours earlier, while the true figure was 0%. That is the failure the endpoint fixes, and
why every number here carries its own age.

Only `utilization.limits[]` is parsed, never its siblings. `five_hour`, `nimbus_quill`,
`iguana_necktie` and friends are internal codenames that churn; the array is
self-describing and carries its own severity.

## Things it will not do

**Show a number it cannot stand behind.** A window that has already reset shows `—`
rather than its last reading, because that reading describes a counter which has gone
back to zero. A window reporting no reset time gets no clock — the two weekly windows do
reset together to the millisecond, so borrowing one for the other would almost certainly
be right, and almost is not a claim the API made.

**Vouch for a number it did not get.** Staleness is tracked per limit, not per segment.
stdin covers `S` and `W` and nothing else, so when a fresh payload arrives beside a
six-hour-old cache, the model-scoped number stays grey and keeps the `?`.

**Invent a severity.** Wherever the response carries a `severity` field it is used as
sent — Anthropic knows when 78% is a warning better than a threshold invented here. Only
the context window and stdin's rate limits arrive without one; those fall back to 0–49
normal, 50–89 warning, 90 and up critical.

**Fail loudly.** Missing file, moved schema, unreadable JSON: print nothing, exit 0. A
statusline that reports its own errors corrupts the console it is drawn in. Every field
is `#[serde(default)]`, so a schema that moves costs one limit rather than the segment.

**Block.** The network refresh runs in a detached child, so a render never waits on a TLS
handshake — it prints what is already on disk and the child improves what the next render
prints. stdin is read only when it is a pipe, and with a deadline, so running
`aimeter line` by hand cannot hang.

## Labels

`S` is the 5-hour session window, `W` the 7-day window across all models, and `@` plus a
model's initial is the 7-day window scoped to that model. The `@` is what distinguishes
the scoped window — not the letter — so a Sonnet-scoped `@S` can never be read as the
session's `S`.

Reasoning effort is one letter: `L` `M` `H` `X`, and `MAX` spelled out because `medium`
already took `M`. A level it does not recognise prints nothing; a wrong letter is worse
than none.

## Environment

| Variable | Effect |
|---|---|
| `AIMETER_NO_FETCH` | never read the token or call the endpoint |
| `AIMETER_REFRESH_SECS` | how stale the limits may get before a background refresh (60) |
| `NO_COLOR` | print the segment without escape codes |
| `AIMETER_BIN` | where the bundled statusline script should look for the binary |

`aimeter line --bar` adds a fill block beside each percentage. Off by default: the digits
already say what the block would, and only the digits are precise.

## Not a plugin

Claude Code plugins can declare skills, agents, hooks, MCP and LSP servers and monitors.
The main `statusLine` is not on that list, and Claude Code runs exactly one of them — so
this is a binary you compose into your own statusline script, not something you install
into the plugin system. `install.sh` does that composing for you.

## Not here on purpose

- **Cost in dollars.** Zero on a Max subscription, and marginal cost is zero besides.
- **A daemon, a watcher, a database.** One process, gone two milliseconds later.
- **Token history.** AImeter tallied every transcript under `~/.claude/projects` into a
  rollup for a while, to drive a TUI. The TUI went and the tally went with it rather than
  living on as a second product inside a repo that does one thing. Both are in the git
  history, along with the four rules that made the tally agree with Claude Code's own
  numbers to the token.
- **Refreshing the OAuth token.** Reading it is already the compromise; writing it would
  be a second one, and one that could break your editor.

## Working on it

Rust is pinned in `.tool-versions`. CI runs these four on Linux and macOS.

```bash
cargo build --release && cargo test
cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

`segment.svg` is generated. Run `python3 tools/segment-svg.py` after changing the
segment's shape or palette, so the diagram cannot drift from what the binary prints.

Four source files, about a thousand lines: `line.rs` renders the segment, `limits.rs`
parses whatever the API or cache hands over, `fetch.rs` talks to the endpoint, `main.rs`
is the two commands.
