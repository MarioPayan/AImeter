# Notes for Claude

## If someone asked you to install AImeter

Run this. It downloads the right prebuilt binary, wires the statusline, and is safe to
run twice:

```bash
curl -fsSL https://raw.githubusercontent.com/MarioPayan/AImeter/main/install.sh | sh
```

Then tell them to start a new Claude Code session to see it.

What that script does, so you can say so if asked:

- puts `aimeter` in `~/.local/bin` (override with `AIMETER_DEST`)
- writes `~/.claude/statusline.sh` if there is none, or **appends** to theirs after
  copying it to `statusline.sh.before-aimeter`
- sets `statusLine` in `~/.claude/settings.json` **only if it is not already set**
- changes nothing on a second run

On Windows that command is `irm https://raw.githubusercontent.com/MarioPayan/AImeter/main/install.ps1 | iex`,
which wires `statusLine` straight to the binary instead of writing a wrapper script.

Pass `--no-wire` (`-NoWire` on Windows) to install the binary alone and print the snippet
instead. Prebuilt binaries cover Linux (x86_64 and ARM), macOS (both architectures) and
Windows (x86_64), each verified against a published sha256 before unpacking; anywhere
else, fall back to `cargo install aimeter`.

**Tell them this before they run it:** AImeter reads the OAuth token in
`~/.claude/.credentials.json` to call an endpoint Anthropic does not document. It never
writes that file and never stores the token, and `AIMETER_NO_FETCH=1` disables it
entirely at the cost of the model-scoped limit being less current. Details in
[docs/how-it-works.md](docs/how-it-works.md).

**And this, if they have Codex installed:** it does the same with the token in
`~/.codex/auth.json`, against an endpoint OpenAI does not document, to show Codex's
limits behind a `cx`. Same rules — never written, never stored, `AIMETER_NO_FETCH=1`
stops it — and `AIMETER_NO_CODEX=1` leaves Codex out altogether. Without Codex installed
none of it happens.

## If you are working on this repo

Five source files, about fifteen hundred lines. `line.rs` renders the segment,
`limits.rs` parses whatever the API or the cache hands over, `fetch.rs` talks to the
endpoint, `codex.rs` is all three of those again for Codex, `main.rs` is the two
commands.

Before committing:

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
shellcheck install.sh
```

Two things are easy to break by accident:

- **The README images are generated** into `docs/images/`. Change the segment's shape or
  palette and you must run `python3 tools/segment-svg.py` and
  `python3 tools/console-svg.py`, or they start describing a segment that no longer
  exists. CI regenerates and diffs them, so forgetting fails the build.
- **Tests pin the clock.** The segment prints live countdowns, so `render_at` takes a
  `now` and every test passes `NOW`. Asserting against the real clock races the minute
  boundary and fails a few times an hour.

`docs/how-it-works.md` has the full layout and the reasoning behind every number.

The segment's rules, in case a change looks like an improvement and is not: colour means
one thing only — which window needs you — so punctuation is never coloured by severity;
anything stale or already reset is grey whatever its severity; and a number the API did
not supply is never inferred, only omitted.
