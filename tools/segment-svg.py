#!/usr/bin/env python3
"""Generate segment.svg — one annotated diagram of the AImeter statusline segment.

Two rules shape it. Direct fill= attributes rather than a <style> block, because
GitHub serves README SVGs through its image proxy into an <img> and inline
attributes are the one thing guaranteed to survive that. And every token is placed
at an explicit x with its own textLength, so character positions are exact
whatever monospace font the viewer has — which is what lets the callout lines
actually point at the thing they label.
"""
import pathlib
from xml.sax.saxutils import escape

OUT = pathlib.Path(__file__).resolve().parent.parent / "segment.svg"

MONO = "ui-monospace, SFMono-Regular, Menlo, Consolas, 'DejaVu Sans Mono', monospace"
BG, EDGE, HAIR = "#17181c", "#2a2c33", "#3a3d46"
TAG, MODEL, DIM = "#5f87af", "#87afaf", "#808080"
OK, WARN, CRIT = "#5faf5f", "#d7af5f", "#d75f5f"
LABEL, HEAD = "#9aa0aa", "#6c7280"

W, H = 900, 422
SIZE = 17.0
ADV = SIZE * 0.6           # monospace advance, forced by textLength below
X0, BASE = 132.0, 168.0    # left edge of the segment, and its baseline

out = []

# The segment, as (text, colour) tokens laid end to end.
TOKENS = [
    ("◈", TAG), (" ", DIM), ("Opus 5", MODEL), ("·X", DIM), (" · ", DIM),
    ("23%", OK), (" │ ", DIM),
    ("S", OK), ("/", DIM), ("4%", OK), (" ", DIM), ("↺2h11", DIM), ("  ", DIM),
    ("W", WARN), ("/", DIM), ("77%", WARN), (" ", DIM), ("↺3d", DIM), ("  ", DIM),
    ("@F", CRIT), ("/", DIM), ("100%", CRIT), (" ", DIM), ("↺3d", DIM),
]


def draw_segment():
    """Place each token at its own x. Returns {token index: (x_start, x_end)}."""
    spans, col = {}, 0
    for i, (s, fill) in enumerate(TOKENS):
        x = X0 + col * ADV
        width = len(s) * ADV
        spans[i] = (x, x + width)
        if s.strip():
            out.append(
                f'<text x="{x:.1f}" y="{BASE}" font-family="{MONO}" font-size="{SIZE}" '
                f'textLength="{width:.1f}" lengthAdjust="spacing" fill="{fill}" '
                f'xml:space="preserve">{escape(s)}</text>'
            )
        col += len(s)
    return spans


def label(x, y, s, size=12, fill=LABEL, anchor="middle"):
    out.append(
        f'<text x="{x:.1f}" y="{y}" font-family="{MONO}" font-size="{size}" '
        f'text-anchor="{anchor}" fill="{fill}" xml:space="preserve">{escape(s)}</text>'
    )


def callout(centre, text_y, tick_from, tick_to):
    """A hairline from a label to the token it names."""
    out.append(
        f'<line x1="{centre:.1f}" y1="{tick_from}" x2="{centre:.1f}" y2="{tick_to}" '
        f'stroke="{HAIR}" stroke-width="1"/>'
    )
    return text_y


spans = draw_segment()
mid = lambda i: (spans[i][0] + spans[i][1]) / 2          # noqa: E731
span = lambda a, b: (spans[a][0] + spans[b][1]) / 2      # noqa: E731

out.insert(0, f'<rect x="0" y="0" width="{W}" height="{H}" rx="7" fill="{BG}"/>')
out.insert(1, f'<rect x="0.5" y="0.5" width="{W-1}" height="{H-1}" rx="7" fill="none" stroke="{EDGE}"/>')

# ── what this line is ────────────────────────────────────────────────────────
label(28, 40, "AImeter — your Claude Code limits, in the statusline", 12.5, HEAD, "start")
out.append(f'<line x1="28" y1="52" x2="{W-28}" y2="52" stroke="{EDGE}"/>')

# ── callouts above ───────────────────────────────────────────────────────────
callout(mid(2), 0, 98, BASE - 26)                       # model
label(mid(2), 90, "model", 12)
callout(mid(5), 0, 98, BASE - 26)                       # context window
label(mid(5), 90, "context window", 12)
callout(mid(3), 0, 130, BASE - 26)                      # effort
label(mid(3), 122, "effort", 12)

# ── callouts below ───────────────────────────────────────────────────────────
callout(span(7, 9), 0, BASE + 10, 206)                  # S/4%
label(span(7, 9), 222, "session · 5h", 12)
callout(mid(13), 0, BASE + 10, 206)                     # W
label(mid(13), 222, "this week", 12)
callout(mid(11), 0, BASE + 10, 244)                     # ↺2h11
label(mid(11), 260, "resets in", 12)
callout(span(19, 21), 0, BASE + 10, 244)                # @F/100%
label(span(19, 21), 260, "this week, one model", 12)

# ── the strip: colour, effort, and the rest, side by side ────────────────────
STRIP = 300
out.append(f'<line x1="28" y1="{STRIP - 16}" x2="{W-28}" y2="{STRIP - 16}" stroke="{EDGE}"/>')

for x, head in ((28, "COLOUR"), (352, "EFFORT"), (600, "ALSO SEEN")):
    label(x, STRIP, head, 10, HEAD, "start")

# colour: swatch, then the word, in one row
for i, (colour, word) in enumerate(
    ((OK, "fine"), (WARN, "close"), (CRIT, "at the limit"), (DIM, "stale or reset"))
):
    y = STRIP + 22 + i * 19
    out.append(f'<rect x="28" y="{y - 9}" width="10" height="10" rx="2" fill="{colour}"/>')
    label(46, y, word, 12, LABEL, "start")

# effort: the five marks, each on its own row against its level
for i, (mark, lvl) in enumerate(
    (("·L", "low"), ("·M", "medium"), ("·H", "high"), ("·X", "xhigh"), ("·MAX", "max"))
):
    y = STRIP + 22 + i * 19
    label(352, y, mark, 12.5, DIM, "start")
    label(400, y, lvl, 12, LABEL, "start")

# everything else a window can show
for i, (glyph, meaning) in enumerate(
    ((["—"], "already reset"), (["?"], "over 6h old"),
     (["no ↺"], "no reset time given"), (["▁▄█"], "--bar, if you want it"))
):
    y = STRIP + 22 + i * 19
    label(600, y, glyph[0], 12.5, DIM, "start")
    label(652, y, meaning, 12, LABEL, "start")

svg = (
    f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" '
    f'role="img" aria-label="The AImeter statusline segment, annotated: the model, its '
    f'reasoning effort and how full the context window is, then the 5-hour, weekly and '
    f'model-scoped limits with how much is spent and when each resets.">'
    + "".join(out) + "</svg>\n"
)
OUT.write_text(svg)
print(f"wrote {OUT}  {W}x{H}")
