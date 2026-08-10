#!/usr/bin/env python3
"""Generate console.svg — the segment where it actually lives, at the bottom of a session.

A stylised terminal rather than a screenshot: the session above the statusline is
illustrative filler, drawn plainly so nobody mistakes it for a capture of anyone's
UI. The only part that is exact is the segment itself, which is placed token by
token in the real xterm-256 colours the binary emits.
"""
import pathlib
from xml.sax.saxutils import escape

OUT = pathlib.Path(__file__).resolve().parent.parent / "docs" / "images" / "console.svg"

MONO = "ui-monospace, SFMono-Regular, Menlo, Consolas, 'DejaVu Sans Mono', monospace"
BG, CHROME, EDGE, RULE = "#17181c", "#1e2027", "#2a2c33", "#262931"
TAG, MODEL, DIM = "#5f87af", "#87afaf", "#808080"
OK, WARN, CRIT = "#5faf5f", "#d7af5f", "#d75f5f"
BODY, FAINT, PROMPT = "#c3c7cd", "#727783", "#7ba6cd"

W, H = 900, 306
SIZE = 14.5
ADV = SIZE * 0.6
PAD = 26

out = []


def text(x, y, s, fill=BODY, size=SIZE, weight="normal"):
    out.append(
        f'<text x="{x:.1f}" y="{y}" font-family="{MONO}" font-size="{size}" '
        f'font-weight="{weight}" fill="{fill}" xml:space="preserve">{escape(s)}</text>'
    )


def tokens(x0, y, parts, size=SIZE):
    """Each token at its own x, with textLength, so colour boundaries land exactly."""
    col = 0
    for s, fill in parts:
        if s.strip():
            out.append(
                f'<text x="{x0 + col * (size * 0.6):.1f}" y="{y}" font-family="{MONO}" '
                f'font-size="{size}" textLength="{len(s) * size * 0.6:.1f}" '
                f'lengthAdjust="spacing" fill="{fill}" xml:space="preserve">{escape(s)}</text>'
            )
        col += len(s)


# ── window ───────────────────────────────────────────────────────────────────
out.append(f'<rect x="0" y="0" width="{W}" height="{H}" rx="8" fill="{BG}"/>')
out.append(f'<path d="M0 8a8 8 0 0 1 8-8h{W-16}a8 8 0 0 1 8 8v30H0z" fill="{CHROME}"/>')
out.append(f'<rect x="0.5" y="0.5" width="{W-1}" height="{H-1}" rx="8" fill="none" stroke="{EDGE}"/>')
out.append(f'<line x1="0" y1="38" x2="{W}" y2="38" stroke="{EDGE}"/>')
text(PAD, 25, "~/projects/api", FAINT, 12.5)

# ── an illustrative session ──────────────────────────────────────────────────
y = 78
text(PAD, y, ">", PROMPT)
text(PAD + 2 * ADV, y, "add a retry to the upload path")

y += 34
for label, detail in (
    ("Read", "src/upload.rs"),
    ("Edit", "src/upload.rs   +12 −3"),
):
    text(PAD + 2 * ADV, y, label, FAINT)
    text(PAD + 9 * ADV, y, detail)
    y += 24

y += 10
text(PAD + 2 * ADV, y, "Bounded retry with backoff, capped at three attempts.", FAINT)

y += 40
text(PAD, y, ">", PROMPT)
out.append(
    f'<rect x="{PAD + 2 * ADV:.1f}" y="{y - 11}" width="8" height="15" fill="{BODY}" opacity="0.5"/>'
)

# ── the statusline row ───────────────────────────────────────────────────────
BAR = H - 34
out.append(f'<line x1="0" y1="{BAR - 22}" x2="{W}" y2="{BAR - 22}" stroke="{RULE}"/>')
tokens(PAD, BAR, [
    ("◈", TAG), (" ", DIM), ("Opus 5", MODEL), ("·X", DIM), (" · ", DIM), ("23%", OK),
    (" │ ", DIM),
    ("S", OK), ("/", DIM), ("4%", OK), (" ", DIM), ("↺2h11", DIM), ("  ", DIM),
    ("W", WARN), ("/", DIM), ("77%", WARN), (" ", DIM), ("↺3d", DIM), ("  ", DIM),
    ("@F", CRIT), ("/", DIM), ("100%", CRIT), (" ", DIM), ("↺3d", DIM),
], size=15)

svg = (
    f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" '
    f'role="img" aria-label="A terminal window with a short coding session, and the AImeter '
    f'segment along the bottom: the model, its reasoning effort and context window, then the '
    f'5-hour, weekly and model-scoped limits with their reset countdowns.">'
    + "".join(out) + "</svg>\n"
)
OUT.write_text(svg)
print(f"wrote {OUT}  {W}x{H}")
