# aimeter

AI coding-tool usage, where you already look.

A statusline segment showing your session, weekly and per-model limits. No daemon, no
database, nothing to keep running — one binary that prints one line in a couple of
milliseconds and exits.

**Claude Code is the only provider today.** The name is the room to add others, not a
claim that they exist: there is no provider abstraction yet, because one implementation
does not tell you where the seam goes. The second provider is what will show that, and
`limits.rs` is where it would land.

```
◈ Opus 5·X · 37% │ S/12% ↺34m  W/2% ↺6d  @F/0%
╰──────┬───────╯   ╰──────────────┬─────────────╯
  this session,              your allowance,
    right now                    over time
```

## Install

Nothing to build. **Ask Claude Code:**

> Install aimeter from github.com/MarioPayan/aimeter and wire it into my statusline

Or do it yourself — this downloads the prebuilt binary for your platform, puts it in
`~/.local/bin`, and prints the snippet to add:

```bash
curl -fsSL https://raw.githubusercontent.com/MarioPayan/aimeter/main/install.sh | sh
```

It is thirty lines and worth reading before you pipe it anywhere, which you can do at
[install.sh](install.sh). It installs a binary and prints instructions; it does not touch
your statusline script, because Claude Code runs a single `statusLine` command and
silently rewriting the file that produces it is not an installer's business.

From source, if you would rather:

```bash
cargo install --git https://github.com/MarioPayan/aimeter
```

## Reading the segment

![Every part of the segment, the colours and what they mean, the reasoning-effort marks, and every value a window can show](segment.svg)

A few of those deserve the reasoning behind them.

**`@S` is the Sonnet-scoped window and never the session.** The scoped label is `@` plus
the model's initial, and the `@` is what distinguishes it — not the letter — so a model
whose name starts with `S` cannot be mistaken for the 5-hour window.

**`MAX` spells itself out** because `medium` already took `M`. An effort level this does
not recognise prints nothing at all; a wrong letter is worse than none.

**Severity is the API's judgement, not ours.** Wherever the response carries a `severity`
field it is used as sent — Anthropic knows when 78% is a warning better than a threshold
invented here. Only two numbers arrive without one, the context window and stdin's rate
limits, and those fall back to 0–49 normal, 50–89 warning, 90 and up critical.

**Colour has exactly one job:** saying which window needs you. So punctuation never takes
a severity colour, and anything stale or already reset goes grey whatever its severity —
a red 100% that is six hours old is a claim this cannot support.

Set `NO_COLOR` to drop the escapes entirely. The `—`, the `?` and the countdowns are all
text rather than colour precisely so they survive that.

## What it reads, and what that costs you

Two of the three sources are files Claude Code already writes. The third is a network
call, and it is the one to make a decision about:

| Source | What it gives | What it needs |
|---|---|---|
| The statusline payload on stdin | model, effort, context window, plus `S` and `W` current every render | nothing |
| `GET /api/oauth/usage` | all windows including the model-scoped one | your Claude Code OAuth token |
| `~/.claude.json` → `cachedUsageUtilization` | the same object, as of whenever Claude Code last refreshed it | nothing |

The payload carries about fifteen fields; four are read. `context_window.used_percentage`
and `effort.level` are free — they arrive on every render and need no credentials — and
they are the only two that describe a ceiling you are moving toward or the reason you are
moving toward it quickly. Cost is not read: on a Max subscription `total_cost_usd` is `0`.

> **The usage endpoint is undocumented, and reaching it means reading your token.**
> `aimeter fetch` reads the access token out of `~/.claude/.credentials.json` and sends
> it as a bearer token to an endpoint Anthropic publishes for its own client, not for
> third parties. It works today and may stop without notice.
>
> The token is **read and never written** — aimeter does not refresh it, does not touch
> the credentials file, and on a 401 falls back to the cache and lets Claude Code sort
> its own token out. Refreshing it here would risk invalidating the one your editor is
> using. The token is never logged and never persisted; only the `limits[]` array from
> the response is written to disk.
>
> **`AIMETER_NO_FETCH=1` turns it off entirely** — the token is never read and the
> endpoint is never called. Everything still works: stdin covers `S` and `W`
> and Claude Code's cache covers the rest. Only the model-scoped limit gets
> less current. (A long `AIMETER_REFRESH_SECS` is *not* an off switch — with nothing
> cached, the first run fetches regardless.)

The cached file is what it says: **a cache.** `fetchedAtMs` sat unchanged through
thirty minutes of continuous API traffic, and was once observed 15.5 hours stale,
reporting 78% for a weekly window that had reset six hours earlier while the true
figure was 0%. That is why the endpoint exists in the first place, and why every
number carries its own age.

Only the `limits[]` array is parsed, never its siblings — `five_hour`, `nimbus_quill`,
`iguana_necktie` and friends are internal codenames that churn, while the array is
self-describing and carries its own severity.

## Working on it

Rust is pinned via `.tool-versions` (asdf). CI runs the same four checks on Linux and
macOS, so if these pass locally they pass there.

```bash
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`segment.svg` is generated rather than hand-drawn. Run `python3 tools/segment-svg.py`
after changing the segment's shape or palette, so the legend cannot quietly drift from
what the binary actually prints.

## The statusline segment

`aimeter line` prints one segment and exits.

**This is not a Claude Code plugin, and cannot be one.** A plugin can declare skills,
agents, event hooks, MCP and LSP servers and monitors — the main `statusLine` is not on
that list. Claude Code runs exactly one `statusLine` command, so you compose it yourself
alongside whatever else you run:

```bash
# ~/.claude/statusline.sh
u=$(bash /path/to/aimeter/statusline/aimeter-statusline.sh 2>/dev/null)
out="${out:+$out  }${u}"
printf '%s' "$out"
```

The script finds the binary at `~/.local/bin/aimeter`, or beside itself in
`target/release/`, or wherever `AIMETER_BIN` points.

Two properties it will not trade away, because it runs on every render forever:

- **Silent on failure.** Missing file, moved schema, bad JSON: prints nothing, exits 0.
  A statusline that reports its own errors corrupts the console it is drawn in.
- **Fast.** ~2 ms. For comparison, anything that spawns `node` costs 60–100 ms.

The network refresh runs in a detached child, so the segment prints what is already on
disk and never waits for a TLS handshake. It also never reads stdin from a terminal:
run `aimeter line` by hand and an unguarded read would hang waiting for Ctrl-D.

Staleness is tracked **per limit**, not per segment. The stdin payload covers `S` and `W`
and nothing else, so when it arrives beside an old cache the model-scoped number stays
greyed out and the segment keeps its trailing `?`. Set `NO_COLOR` to drop the escapes;
the `?` is what survives.

A window that has already reset shows `S/—` rather than its last reading, because that
number describes a counter which has since gone back to zero. A window that reports no
`resets_at` at all — the model-scoped one does this in the wild — simply gets no clock.
The two weekly windows do reset together to the millisecond, so borrowing one for the
other would almost certainly be right, which is exactly why it is not done: almost is
not a claim the API made.

`aimeter line --bar` adds a fill block beside each percentage (`S/12%▁`). Off by default,
because the digits already say what the block would and only the digits are precise.

## Not here on purpose

- **Cost in dollars.** On a Max subscription `modelUsage.costUSD` is `0` and marginal
  cost is zero; percent-of-limit is the real currency. The usage endpoint does return a
  `spend` object; it is dropped rather than written to disk.
- **A daemon, a watcher, a database.** One process, started by the statusline, gone
  two milliseconds later.
- **Token history.** aimeter tallied every transcript under `~/.claude/projects` into a
  rollup for a while, to drive a TUI. The TUI went; the tally went with it rather than
  living on as a second product inside a repo that does one thing. It is in the git
  history — `src/rollup.rs`, deleted in the commit that removed the dashboard — with the
  four rules that made it agree with Claude Code's own numbers to the token.
- **Refreshing the OAuth token.** Reading it is already the compromise; writing it would
  be a second one, and one that could break your editor.

## License

MIT — see [LICENSE](LICENSE).
