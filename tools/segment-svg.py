#!/usr/bin/env python3
"""Generate segment.svg — a legend for the aimeter statusline segment.

Direct fill= attributes rather than a <style> block: GitHub serves README SVGs
through its image proxy into an <img>, and inline attributes are the one thing
guaranteed to survive that.
"""
import pathlib
from xml.sax.saxutils import escape

OUT = pathlib.Path(__file__).resolve().parent.parent / "segment.svg"

MONO = "ui-monospace, SFMono-Regular, Menlo, Consolas, 'DejaVu Sans Mono', monospace"

BG, EDGE = "#17181c", "#2a2c33"
TAG, MODEL, DIM = "#5f87af", "#87afaf", "#808080"
OK, WARN, CRIT = "#5faf5f", "#d7af5f", "#d75f5f"
HEAD, NOTE, PLAIN = "#6c7280", "#9aa0aa", "#cfd2d6"

W = 760
out = []


def text(x, y, parts, size=14.5, anchor="start"):
    """parts: [(string, fill), ...] laid out sequentially in one <text>."""
    spans = "".join(f'<tspan fill="{f}">{escape(s)}</tspan>' for s, f in parts)
    out.append(
        f'<text x="{x}" y="{y}" font-family="{MONO}" font-size="{size}" '
        f'text-anchor="{anchor}" xml:space="preserve">{spans}</text>'
    )


def header(y, label):
    out.append(
        f'<text x="24" y="{y}" font-family="{MONO}" font-size="10.5" '
        f'letter-spacing="1.4" fill="{HEAD}">{escape(label)}</text>'
    )
    out.append(f'<line x1="24" y1="{y + 9}" x2="{W - 24}" y2="{y + 9}" stroke="{EDGE}"/>')


def row(y, parts, meaning):
    text(40, y, parts)
    out.append(
        f'<text x="270" y="{y}" font-family="{MONO}" font-size="12.5" '
        f'fill="{NOTE}" xml:space="preserve">{escape(meaning)}</text>'
    )


def swatch(y, colour, parts, meaning):
    out.append(f'<rect x="24" y="{y - 11}" width="12" height="12" rx="2" fill="{colour}"/>')
    text(48, y, parts)
    out.append(
        f'<text x="270" y="{y}" font-family="{MONO}" font-size="12.5" '
        f'fill="{NOTE}" xml:space="preserve">{escape(meaning)}</text>'
    )


# ── the whole segment, once, for reference ───────────────────────────────────
header(34, "THE SEGMENT")
text(24, 66, [
    ("◈", TAG), (" ", DIM), ("Opus 5", MODEL), ("·X", DIM), (" · ", DIM), ("23%", OK),
    (" │ ", DIM), ("S", OK), ("/", DIM), ("4%", OK), (" ↺2h11  ", DIM),
    ("W", WARN), ("/", DIM), ("77%", WARN), (" ↺3d  ", DIM),
    ("@F", CRIT), ("/", DIM), ("100%", CRIT), (" ↺3d", DIM),
], size=16)

# ── every part, in isolation ─────────────────────────────────────────────────
header(108, "EACH PART")
parts = [
    ([("◈", TAG)], "the segment mark"),
    ([("Opus 5", MODEL)], "the model you are talking to"),
    ([("·X", DIM)], "reasoning effort"),
    ([("23%", OK)], "how full the context window is"),
    ([("│", DIM)], "divider — this session ends, your allowance begins"),
    ([("S", OK)], "session window, 5 hours"),
    ([("W", OK)], "week window, 7 days, all models"),
    ([("@F", OK)], "week window scoped to one model — @ plus its initial"),
    ([("/", DIM)], "separator between a window and its number"),
    ([("↺2h11", DIM)], "when that window resets"),
]
y = 134
for p, m in parts:
    row(y, p, m)
    y += 23

# ── colour ───────────────────────────────────────────────────────────────────
header(y + 24, "COLOUR — one job: which window needs you")
y += 50
for colour, sample, meaning in [
    (OK, [("S", OK), ("/", DIM), ("4%", OK)], "room to spare"),
    (WARN, [("W", WARN), ("/", DIM), ("77%", WARN)], "warning — the API's own judgement, not a threshold"),
    (CRIT, [("@F", CRIT), ("/", DIM), ("100%", CRIT)], "critical — likewise"),
    (DIM, [("W", DIM), ("/", DIM), ("77%", DIM), (" ↺3d", DIM)], "structure, or a number that is stale or already reset"),
]:
    swatch(y, colour, sample, meaning)
    y += 23
out.append(
    f'<text x="24" y="{y + 8}" font-family="{MONO}" font-size="12" fill="{HEAD}" '
    f'xml:space="preserve">Punctuation is never coloured by severity. Anything stale or reset is always grey.</text>'
)
y += 8

# ── effort ───────────────────────────────────────────────────────────────────
header(y + 34, "REASONING EFFORT")
y += 60
for mark, meaning in [("L", "low"), ("M", "medium"), ("H", "high"), ("X", "xhigh"), ("MAX", "max")]:
    row(y, [("·" + mark, DIM)], meaning)
    y += 23
out.append(
    f'<text x="24" y="{y + 8}" font-family="{MONO}" font-size="12" fill="{HEAD}" '
    f'xml:space="preserve">A level it does not recognise prints nothing.</text>'
)
y += 8

# ── values ───────────────────────────────────────────────────────────────────
header(y + 30, "VALUES")
y += 56
for sample, meaning in [
    ([("4%", OK)], "percent of the window spent"),
    ([("—", DIM)], "already reset — that counter is gone, so no number is shown"),
    ([("↺18m", DIM)], "resets in 18 minutes — minutes, under an hour"),
    ([("↺2h11", DIM)], "resets in 2h11 — hours and minutes, under a day"),
    ([("↺6d", DIM)], "resets in 6 days — whole days beyond that"),
    ([("(none)", HEAD)], "no clock at all: that window reported no reset time"),
    ([("?", DIM)], "something on the line is more than six hours old"),
    ([("▁▂▃▄▅▆▇█", DIM)], "the --bar fill, eight levels across 0–100%"),
]:
    row(y, sample, meaning)
    y += 23

H = y + 14
svg = (
    f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" '
    f'viewBox="0 0 {W} {H}" role="img" aria-label="A legend for the aimeter '
    f'statusline segment: each part, what the colours mean, the reasoning-effort '
    f'marks, and every value a window can show.">'
    f'<rect x="0" y="0" width="{W}" height="{H}" rx="6" fill="{BG}"/>'
    f'<rect x="0.5" y="0.5" width="{W - 1}" height="{H - 1}" rx="6" fill="none" stroke="{EDGE}"/>'
    + "".join(out)
    + "</svg>\n"
)
open(OUT, "w").write(svg)
print(f"wrote {OUT}  {W}x{H}")
