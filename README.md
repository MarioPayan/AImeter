# ccmeter

Claude Code usage, where you already look.

A statusline segment showing your session, weekly and per-model limits, and a TUI for
the history behind them. It reads the files Claude Code already writes — no API calls,
no tokens, no daemon, nothing to keep running.

```
[PONYTAIL]  [KAIZEN] 38 awaiting you  [USAGE] 5h 4% · 7d 77% · Fable 100%
                                              green   amber    red
```

## Build

Rust is pinned via `.tool-versions` (asdf).

```bash
cargo build --release
ln -sf "$PWD/target/release/ccmeter" ~/.local/bin/ccmeter
ccmeter backfill      # ~0.5s, once
```

## The statusline segment

`ccmeter line` prints one segment and exits. Claude Code has a single `statusLine`
command, so compose it with whatever else you run — plugins cannot declare one:

```bash
# ~/.claude/statusline.sh
u=$(bash "$HOME/repos/Kaze/ccmeter/hooks/ccmeter-statusline.sh" 2>/dev/null)
out="${out:+$out  }${u}"
printf '%s' "$out"
```

Two properties it will not trade away, because it runs on every render forever:

- **Silent on failure.** Missing file, moved schema, bad JSON: prints nothing, exits 0.
  A statusline that reports its own errors corrupts the console it is drawn in.
- **Fast.** ~2 ms. For comparison, anything that spawns `node` costs 60–100 ms.

It also never reads stdin. Claude Code pipes JSON in, nothing here needs it, and a
read on a terminal stdin would hang the first time you ran `ccmeter line` by hand.

## The TUI

`ccmeter` (or `ccmeter tui`) opens the dashboard: limit gauges with reset countdowns,
today's tokens per model, a daily sparkline, and a model × window table.

```
╭ Limits ────────────────────────────────────────────────────────────────╮
│5h              ███░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░   4%  in 2h33 │
│7d              ███████████████████████████░░░░░░░░░░░░░░  77%  in 13h03│
│Fable           ██████████████████████████████████████████ 100%  in 13h03│
╰────────────────────────────────────────────────────────────────────────╯
```

`q` quit · `r` refresh · `t` theme · `?` help

No watcher and no server: it re-reads the limits every 2 s and the transcripts every
5 s, which costs about a millisecond each. A `notify` thread would buy nothing.

## Where the numbers come from

| Panel | Source |
|---|---|
| Limits | `~/.claude.json` → `cachedUsageUtilization.utilization.limits[]` |
| Tokens | `~/.claude/projects/**/*.jsonl`, rolled up into `~/.claude/ccmeter/rollup.json` |

Only the `limits[]` array is parsed, never its siblings — `five_hour`, `nimbus_quill`,
`iguana_necktie` and friends are internal codenames that churn, while the array is
self-describing and carries its own severity.

That file is a **cache**: `fetchedAtMs` sat unchanged through thirty minutes of
continuous API traffic, so Claude Code refreshes it on its own slow cadence. The
numbers are shown in colour anyway, with their age in the TUI header, and are only
marked stale past six hours — the point at which the shortest window (five hours) has
certainly rolled over and the number is wrong rather than merely old.

### The tally

Four rules, each established by reconciling against `~/.claude/stats-cache.json` until
two independent parsers agreed to the token. `cargo test` re-runs that comparison
against every day Claude Code has cached:

- **Walk recursively, but not into `subagents/workflows/`.** Only 148 of 739 transcripts
  sit at the top of a project directory; the rest are `<session>/subagents/…`. A flat
  scan reports 27% of the real Fable number. Including *everything* over-counts the days
  that used the Workflow tool — those agents are already in the parent session's books.
  Agreement: flat 18/58 day-model totals, everything 54/58, this rule **58/58**.
- **Do not deduplicate.** Repeated `requestId`/`message.id` pairs are separate accounting
  entries. Collapsing them lands at ~57%.
- **A request costs all four token fields** — input, output, cache read, cache creation.
  Input and output alone are ~0.1% of the truth; cache traffic is almost all of it.
- **Bucket by the record's own timestamp**, in UTC. A session running past midnight
  writes two days into one file, and UTC is what makes the totals match.

## Not here on purpose

- **Cost in dollars.** On a Max subscription `modelUsage.costUSD` is `0` and marginal
  cost is zero; percent-of-limit is the real currency.
- **Fetching limits from the API.** Reading Claude Code's cache needs no credentials.
- **A daemon, a watcher, a database.** A 116 KB JSON rollup and a poll cover it.
