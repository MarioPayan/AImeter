//! The statusline segment: one line, gone in a couple of milliseconds.
//!
//! This runs on every statusline render — every 10 seconds all day, forever. Two
//! constraints follow from that and shape everything here:
//!
//!   - **It never fails loudly.** Missing file, moved schema, unreadable JSON: print
//!     nothing, exit 0. A statusline that emits an error message or a stack trace
//!     corrupts the console it is drawn in, which is a far worse outcome than a
//!     missing segment.
//!   - **It never blocks.** The stdin payload is read only when stdin is a pipe.
//!     Run `aimeter line` by hand and stdin is your keyboard, where an unguarded
//!     read waits for Ctrl-D forever.
//!
//! ```text
//! ◈ Opus 5·X · 37% │ S/12% ↺4h07  W/2% ↺6d  @F/0%
//! ╰──────┬───────╯   ╰──────────────┬──────────────╯
//!   this session                your allowance
//!    right now                    over time
//! ```
//!
//! The divider is the whole layout: left of it is what this conversation is doing —
//! model, reasoning effort, how full the context window is — and right of it is where
//! you stand against limits that outlive it. Two different clocks, so they get two
//! different sides.
//!
//! Three sources feed the right-hand side, in order of authority: the stdin payload
//! Claude Code regenerates every render, then our own fetch from the usage endpoint,
//! then Claude Code's cache. stdin is documented, current, and needs no credentials,
//! so it wins on the numbers it carries — the 5-hour and 7-day windows and nothing
//! else. The model-scoped limit only exists in the other two.
//!
//! Severity normally comes from the API's own field rather than thresholds invented
//! here — Anthropic knows when 78% is a warning better than we do. The exceptions are
//! stdin's rate limits and the context window, which send a percentage and no opinion,
//! so `from_percent` supplies one.

use crate::limits::{Limit, Severity, Snapshot};
use serde::Deserialize;

const TAG: &str = "\x1b[38;5;67m";
const MODEL: &str = "\x1b[38;5;109m";
const DIM: &str = "\x1b[38;5;244m";
const RESET: &str = "\x1b[0m";

/// What the caller wants drawn. Two booleans at a call site are unreadable; named
/// fields are not an abstraction, just a parameter list that says which is which.
#[derive(Clone, Copy, Default)]
pub struct Style {
    pub colour: bool,
    /// `--bar`: a fill block beside each percentage. Off by default — the digits
    /// already say what the block would, and only the digits are precise.
    pub bar: bool,
    /// A newer release exists. One arrow, at the end, in the separator grey —
    /// this is news, not an alarm, and it is sharing a line with numbers that
    /// actually are.
    pub update: bool,
    /// Plain ASCII instead of `◈ │ ↺ ↑ ▁`. Whether a glyph renders is a property
    /// of the viewer's font, which nothing on the far side of a pipe can query —
    /// but a non-UTF-8 locale guarantees breakage, and that *is* detectable, so
    /// this switches on automatically there and by hand (`AIMETER_ASCII`) for
    /// fonts that lie.
    pub ascii: bool,
}

impl Style {
    fn mark(&self) -> &'static str {
        if self.ascii {
            "*"
        } else {
            "◈"
        }
    }
    fn divider(&self) -> &'static str {
        if self.ascii {
            " |"
        } else {
            " │"
        }
    }
    /// Joins the effort mark to the model, and separates identity from context.
    fn dot(&self) -> &'static str {
        if self.ascii {
            "."
        } else {
            "·"
        }
    }
    fn resets(&self) -> &'static str {
        if self.ascii {
            "~"
        } else {
            "↺"
        }
    }
    fn gone(&self) -> &'static str {
        if self.ascii {
            "-"
        } else {
            "—"
        }
    }
    fn arrow(&self) -> &'static str {
        if self.ascii {
            " ^"
        } else {
            " ↑"
        }
    }
}

/* ----------------------------------------------------------------- session ---- */

/// The JSON Claude Code pipes to a statusline command on every render.
///
/// Only the four fields worth having are declared, out of the fifteen or so that
/// arrive. `rate_limits` is documented and current — it beats anything cached and
/// needs no credentials — but carries only the 5-hour and 7-day windows, so a
/// model-scoped limit still has to come from the API.
#[derive(Deserialize, Default)]
pub struct Session {
    #[serde(default)]
    model: ModelInfo,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
    #[serde(default)]
    context_window: Option<ContextWindow>,
    #[serde(default)]
    effort: Option<Effort>,
}

#[derive(Deserialize, Default)]
struct ModelInfo {
    #[serde(default)]
    display_name: String,
}

#[derive(Deserialize, Default)]
struct RateLimits {
    #[serde(default)]
    five_hour: Option<Window>,
    #[serde(default)]
    seven_day: Option<Window>,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct Window {
    #[serde(default)]
    used_percentage: f64,
    /// Unix epoch seconds.
    #[serde(default)]
    resets_at: Option<i64>,
}

/// How full this conversation is. The one ceiling here you collide with hourly, and
/// the only reading in the segment that changes on every single turn.
#[derive(Deserialize, Default)]
struct ContextWindow {
    #[serde(default)]
    used_percentage: Option<f64>,
}

#[derive(Deserialize, Default)]
struct Effort {
    #[serde(default)]
    level: String,
}

/// How long to wait for the payload before giving up on it.
///
/// Claude Code writes the JSON and closes the pipe, so in practice EOF is already
/// there and this never elapses. It exists for every other way `aimeter line` can
/// be invoked.
const STDIN_WAIT: std::time::Duration = std::time::Duration::from_millis(150);

/// Read the payload, or `None` when there isn't one.
///
/// Two guards, because one is not enough. `is_terminal` catches running this by
/// hand, where stdin is your keyboard and a read waits for Ctrl-D forever. The
/// deadline catches the case that guard misses and that actually bit: stdin
/// inherited as an open pipe nobody ever writes to or closes, where `is_terminal`
/// is false and the read blocks just the same.
///
/// The reader thread is left blocked rather than cancelled — there is no portable
/// way to interrupt a blocking read, and the process is about to exit anyway.
pub fn read_session() -> Option<Session> {
    use std::io::{IsTerminal, Read};
    if std::io::stdin().is_terminal() {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut raw = String::new();
        // Capped: a trusted parent, but an unbounded read on a pipe is unbounded.
        let read = std::io::stdin().take(256 * 1024).read_to_string(&mut raw);
        let _ = tx.send(read.ok().map(|_| raw));
    });
    let raw = rx.recv_timeout(STDIN_WAIT).ok()??;
    serde_json::from_str(&raw).ok()
}

impl Session {
    /// `Opus 5 (1M context)` -> `Opus 5`. The parenthetical is a property of the
    /// session, not of the model, and the context window is now shown as a number
    /// anyway — which is the part of "1M context" you actually wanted.
    fn model_name(&self) -> Option<&str> {
        let name = self.model.display_name.split(" (").next()?.trim();
        (!name.is_empty()).then_some(name)
    }

    /// One letter for the reasoning effort, because it is the main reason two
    /// identical-looking sessions burn the windows at different speeds.
    ///
    /// `max` spells itself out rather than taking `M`, which `medium` already has.
    /// An unrecognised level prints nothing — a wrong letter is worse than none.
    ///
    /// `U` is the one letter here nothing prints today: `ultracode` is a real rung of
    /// the ladder — choosing it clears whatever else was set — but Claude Code resolves
    /// it to `xhigh` before it builds the payload, so the level never arrives under its
    /// own name. The arm costs a line and is right the day that changes.
    fn effort_mark(&self) -> Option<&'static str> {
        match self.effort.as_ref()?.level.as_str() {
            "low" => Some("L"),
            "medium" => Some("M"),
            "high" => Some("H"),
            "xhigh" => Some("X"),
            "max" => Some("MAX"),
            "ultracode" => Some("U"),
            _ => None,
        }
    }

    fn context_percent(&self) -> Option<f64> {
        self.context_window.as_ref()?.used_percentage
    }

    /// Fold the stdin windows over whatever the API or cache had, pairing each limit
    /// with whether *it* is stale.
    ///
    /// stdin wins on the numbers because it is regenerated every render, but it sends
    /// no severity, so that is derived. Any limit stdin does not know about — the
    /// model-scoped one — is carried through untouched.
    ///
    /// Staleness is per-limit rather than per-segment, and that distinction is the
    /// whole reason this returns pairs. stdin covers `S` and `W` and nothing else, so
    /// a payload arriving beside a six-hour-old cache leaves the model-scoped limit
    /// exactly as stale as it was. Marking the segment fresh on stdin's word would
    /// paint that number in confident red — the failure the marker exists to prevent.
    fn merge(&self, snapshot: Option<&Snapshot>) -> Vec<(Limit, bool)> {
        let cached_stale = snapshot.is_some_and(|s| s.is_stale());
        let mut limits: Vec<(Limit, bool)> = snapshot
            .map(|s| s.limits.iter().cloned().map(|l| (l, cached_stale)).collect())
            .unwrap_or_default();
        let Some(fresh) = self.rate_limits.as_ref() else { return limits };

        for (label, window) in [("S", fresh.five_hour), ("W", fresh.seven_day)] {
            let Some(window) = window else { continue };
            let resets_at = window
                .resets_at
                .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0).map(|t| t.to_rfc3339()));
            let updated = Limit {
                label: label.to_string(),
                percent: window.used_percentage,
                severity: Severity::from_percent(window.used_percentage),
                resets_at,
            };
            match limits.iter_mut().find(|(l, _)| l.label == label) {
                Some(existing) => *existing = (updated, false),
                None => limits.push((updated, false)),
            }
        }
        limits
    }
}

/* ------------------------------------------------------------------ paint ---- */

fn colour(severity: Severity) -> &'static str {
    match severity {
        Severity::Normal => "\x1b[38;5;71m",
        Severity::Warning => "\x1b[38;5;179m",
        Severity::Critical => "\x1b[38;5;167m",
    }
}

/// Honour `NO_COLOR`, and nothing else.
///
/// Deliberately *not* gated on `is_terminal`: a statusline's stdout is a pipe by
/// construction — Claude Code captures what this prints and draws it itself — so
/// that check would disable colour exactly where it is wanted. `NO_COLOR` is the
/// knob for anyone capturing the segment somewhere colour is unwelcome.
fn use_colour() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

/// Whether to fall back to ASCII glyphs.
///
/// `AIMETER_ASCII` decides when set (`0` forbids, anything else forces). Unset,
/// the locale decides: a terminal not running UTF-8 *will* mangle `◈ ↺ ▁`, and
/// that is the one glyph failure a pipe can actually see coming. Font coverage —
/// tofu in a UTF-8 terminal whose font lacks a codepoint — is invisible from
/// here, which is what the manual override is for.
fn use_ascii() -> bool {
    let get = |k: &str| std::env::var(k).ok();
    match get("AIMETER_ASCII").as_deref().map(str::trim) {
        Some("0") => false,
        Some(v) if !v.is_empty() => true,
        _ => !locale_is_utf8(
            get("LC_ALL").as_deref(),
            get("LC_CTYPE").as_deref(),
            get("LANG").as_deref(),
        ),
    }
}

/// POSIX precedence: `LC_ALL` beats `LC_CTYPE` beats `LANG`, empty counts as
/// unset, and all three unset is the C locale — which is ASCII.
fn locale_is_utf8(lc_all: Option<&str>, lc_ctype: Option<&str>, lang: Option<&str>) -> bool {
    let effective =
        [lc_all, lc_ctype, lang].into_iter().flatten().map(str::trim).find(|s| !s.is_empty());
    effective.is_some_and(|s| {
        let s = s.to_ascii_lowercase();
        s.contains("utf-8") || s.contains("utf8")
    })
}

fn paint(text: &str, code: &str, on: bool) -> String {
    if on {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

/// Where the update arrow points.
const RELEASES_URL: &str = "https://github.com/MarioPayan/AImeter/releases/latest";

/// Wrap text in an OSC 8 hyperlink, so the arrow can be clicked.
///
/// Claude Code passes these through to terminals it detects as supporting them.
/// Everywhere else — Terminal.app, anything older — the sequence is ignored and
/// the text shows exactly as it would have, which is why the arrow has to mean
/// something on its own rather than being a bare "click here".
///
/// BEL-terminated rather than ST, because that is the form with the widest
/// support and the one Claude Code's own examples use. Gated on the same switch
/// as colour: `NO_COLOR` is this segment's one "emit no escape sequences" knob.
fn link(text: &str, url: &str, on: bool) -> String {
    if on {
        format!("\x1b]8;;{url}\x07{text}\x1b]8;;\x07")
    } else {
        text.to_string()
    }
}

/// `▁▂▃▄▅▆▇█` across 0–100%, for `--bar` — or an ASCII ramp of rising density.
fn fill(percent: f64, ascii: bool) -> &'static str {
    const LEVELS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    const PLAIN: [&str; 8] = [".", ":", "-", "=", "+", "*", "#", "%"];
    let i = ((percent / 100.0 * 8.0).floor().max(0.0) as usize).min(7);
    if ascii {
        PLAIN[i]
    } else {
        LEVELS[i]
    }
}

/// `18m`, `2h11`, `6d` — the countdown, at the precision that changes a decision.
///
/// Minutes matter under an hour and are noise over a day, so the unit follows the
/// magnitude. `None` once the window has passed: a countdown never runs backwards,
/// and an expired window has a different thing to say anyway.
fn until(rfc3339: &str, now: i64) -> Option<String> {
    let when = chrono::DateTime::parse_from_rfc3339(rfc3339).ok()?;
    let secs = when.timestamp() - now;
    if secs <= 0 {
        return None;
    }
    Some(match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h{:02}", s / 3600, (s % 3600) / 60),
        s => format!("{}d", s / 86_400),
    })
}

/// One window: `S/12% ↺4h07`.
///
/// The slash and the clock stay in the separator grey while the label and the number
/// take the severity colour. Colour has exactly one job in this segment — saying
/// which window needs you — and punctuation competing for it makes that job harder.
/// The clock is what you read *after* the colour has caught your eye, not instead.
fn window(limit: &Limit, stale: bool, st: Style, now: i64) -> String {
    let expired = limit.expired_at(now);
    // A window that has already reset gets no number at all. Printing the last
    // reading would be worse than printing nothing: it is not an old measurement of
    // the current window, it is a measurement of a window that no longer exists.
    let value =
        if expired { st.gone().to_string() } else { format!("{}%", limit.percent.round() as i64) };
    // Stale numbers are greyed regardless of severity: a red 100% that is actually
    // six hours old is a claim we cannot support.
    let lit = if stale || expired { DIM } else { colour(limit.severity) };

    let mut out = String::with_capacity(32);
    out.push_str(&paint(&limit.label, lit, st.colour));
    out.push_str(&paint("/", DIM, st.colour));
    out.push_str(&paint(&value, lit, st.colour));
    if st.bar && !expired {
        out.push_str(&paint(fill(limit.percent, st.ascii), DIM, st.colour));
    }
    // No clock when the window will not say. `resets_at` comes back null on the
    // model-scoped limit in the wild, and an absent countdown is honest where a
    // borrowed one — the two weekly windows do reset together — would be a guess.
    if !expired {
        if let Some(left) = limit.resets_at.as_deref().and_then(|r| until(r, now)) {
            out.push_str(&paint(&format!(" {}{left}", st.resets()), DIM, st.colour));
        }
    }
    out
}

/// The whole segment, or `None` when there is nothing honest to say.
pub fn render(snapshot: Option<&Snapshot>, session: Option<&Session>, st: Style) -> Option<String> {
    render_at(snapshot, session, st, chrono::Utc::now().timestamp())
}

/// Split out so tests can pin the clock. Countdowns are printed now, so asserting
/// against `now()` would race the minute boundary and fail a few times an hour.
pub fn render_at(
    snapshot: Option<&Snapshot>,
    session: Option<&Session>,
    st: Style,
    now: i64,
) -> Option<String> {
    let default_session = Session::default();
    let session = session.unwrap_or(&default_session);
    let limits = session.merge(snapshot);

    // Left of the divider: this conversation, right now.
    let mut identity = String::new();
    if let Some(model) = session.model_name() {
        identity.push_str(&paint(model, MODEL, st.colour));
        if let Some(mark) = session.effort_mark() {
            identity.push_str(&paint(&format!("{}{mark}", st.dot()), DIM, st.colour));
        }
    }
    if let Some(pct) = session.context_percent() {
        let lit = colour(Severity::from_percent(pct));
        if !identity.is_empty() {
            identity.push_str(&paint(&format!(" {} ", st.dot()), DIM, st.colour));
        }
        identity.push_str(&paint(&format!("{}%", pct.round() as i64), lit, st.colour));
    }

    if limits.is_empty() && identity.is_empty() {
        return None;
    }

    // Right of the divider: your allowance, over time.
    let windows: Vec<String> = limits.iter().map(|(l, stale)| window(l, *stale, st, now)).collect();

    let mut out = paint(st.mark(), TAG, st.colour);
    if !identity.is_empty() {
        out.push(' ');
        out.push_str(&identity);
    }
    if !windows.is_empty() {
        if !identity.is_empty() {
            out.push_str(&paint(st.divider(), DIM, st.colour));
        }
        out.push(' ');
        out.push_str(&windows.join("  "));
    }
    // A trailing "?" is the only marker of staleness that survives a colourless
    // terminal, so it is not merely decoration. One stale number among fresh ones
    // still earns it: the mark says "something here is old", and the greyed-out
    // window says which.
    if limits.iter().any(|(_, stale)| *stale) {
        out.push_str(&paint("?", DIM, st.colour));
    }
    // Last, after the staleness mark, so the two terse end-markers read as a pair
    // and neither moves when the other appears. The space stays outside the link,
    // so only the glyph itself is a click target.
    if st.update {
        out.push(' ');
        out.push_str(&link(
            &paint(st.arrow().trim_start(), DIM, st.colour),
            RELEASES_URL,
            st.colour,
        ));
    }
    Some(out)
}

/// Print the segment. Any failure at all is silence.
///
/// The refresh is kicked off first and deliberately not waited for: this call prints
/// what is already on disk, and the child it spawned improves what the *next* call
/// prints. That is what keeps the statusline in single-digit milliseconds while the
/// numbers stay a minute old at worst.
pub fn main(bar: bool) {
    // Read the payload first: it is the parent's pipe, and leaving it unread while
    // doing other work is how a statusline ends up blocking its own writer.
    let session = read_session();
    crate::fetch::refresh_in_background();
    let st = Style {
        colour: use_colour(),
        bar,
        // A file read, not a network call: whatever the once-a-day check last saw.
        update: crate::fetch::update_available(),
        ascii: use_ascii(),
    };
    if let Some(out) = render(crate::limits::read().as_ref(), session.as_ref(), st) {
        print!("{out}");
    }
}

/* ------------------------------------------------------------------ tests ---- */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::parse;

    /// A fixed "now" every test renders against, so countdowns are assertable.
    /// 2026-08-09T12:00:00Z.
    const NOW: i64 = 1_786_276_800;
    /// NOW + 2h. stdin carries reset times as epoch seconds rather than RFC 3339,
    /// so the payload fixtures below spell this number out; it is named here so the
    /// `↺2h00` they assert is traceable to something.
    const IN_2H: i64 = 1_786_284_000;

    #[test]
    fn the_stdin_fixtures_are_two_hours_ahead_of_the_pinned_clock() {
        assert_eq!(IN_2H - NOW, 2 * 3600);
    }

    /// Reset times are relative to NOW: S in 2h11, W and the scoped one in 3d.
    const FIXTURE: &str = r#"{"cachedUsageUtilization":{"fetchedAtMs":1000,"utilization":{"limits":[
        {"kind":"session","percent":4,"severity":"normal","resets_at":"2026-08-09T14:11:00Z"},
        {"kind":"weekly_all","percent":77,"severity":"warning","resets_at":"2026-08-12T12:00:00Z"},
        {"kind":"weekly_scoped","percent":100,"severity":"critical","resets_at":"2026-08-12T12:00:00Z",
         "scope":{"model":{"display_name":"Fable"}}}
    ]}}}"#;

    fn plain() -> Style {
        Style::default()
    }
    fn lit() -> Style {
        Style { colour: true, ..Style::default() }
    }
    fn session(json: &str) -> Session {
        serde_json::from_str(json).expect("session parses")
    }

    #[test]
    fn renders_three_windows_with_their_countdowns() {
        let snap = parse(FIXTURE, 1000).unwrap();
        assert_eq!(
            render_at(Some(&snap), None, plain(), NOW).unwrap(),
            "◈ S/4% ↺2h11  W/77% ↺3d  @F/100% ↺3d"
        );
    }

    /// The identity block: model, effort, context — all from one stdin payload.
    #[test]
    fn the_identity_block_carries_model_effort_and_context() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let s = session(
            r#"{"model":{"display_name":"Opus 5 (1M context)"},
                "effort":{"level":"xhigh"},
                "context_window":{"used_percentage":37}}"#,
        );
        assert_eq!(
            render_at(Some(&snap), Some(&s), plain(), NOW).unwrap(),
            "◈ Opus 5·X · 37% │ S/4% ↺2h11  W/77% ↺3d  @F/100% ↺3d"
        );
    }

    /// `max` cannot take `M` — `medium` has it — and an effort level we have never
    /// heard of prints nothing rather than a letter we made up.
    #[test]
    fn effort_letters_do_not_collide() {
        let marks =
            |level: &str| session(&format!(r#"{{"effort":{{"level":"{level}"}}}}"#)).effort_mark();
        assert_eq!(marks("low"), Some("L"));
        assert_eq!(marks("medium"), Some("M"));
        assert_eq!(marks("high"), Some("H"));
        assert_eq!(marks("xhigh"), Some("X"));
        assert_eq!(marks("max"), Some("MAX"));
        assert_eq!(marks("ultracode"), Some("U"));
        assert_eq!(marks("telepathic"), None);
        assert_eq!(Session::default().effort_mark(), None);
    }

    /// Context is a ceiling like the others, so it takes the same derived severity.
    #[test]
    fn context_is_coloured_by_how_full_it_is() {
        let full = session(
            r#"{"model":{"display_name":"Opus 5"},"context_window":{"used_percentage":96}}"#,
        );
        let out = render_at(None, Some(&full), lit(), NOW).unwrap();
        assert!(out.contains("\x1b[38;5;167m96%"), "nearly full is red: {out:?}");

        let roomy = session(
            r#"{"model":{"display_name":"Opus 5"},"context_window":{"used_percentage":23}}"#,
        );
        let out = render_at(None, Some(&roomy), lit(), NOW).unwrap();
        assert!(out.contains("\x1b[38;5;71m23%"), "roomy is green: {out:?}");
    }

    /// The scoped label is `@` plus an initial, so a Sonnet-scoped limit cannot be
    /// mistaken for the session window. This is the whole reason for the marker.
    #[test]
    fn the_scoped_window_never_collides_with_the_session() {
        let raw = r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
            {"kind":"session","percent":4,"severity":"normal"},
            {"kind":"weekly_scoped","percent":50,"severity":"warning",
             "scope":{"model":{"display_name":"Sonnet"}}}
        ]}}}"#;
        let snap = parse(raw, 1).unwrap();
        assert_eq!(render_at(Some(&snap), None, plain(), NOW).unwrap(), "◈ S/4%  @S/50%");
    }

    /// stdin is regenerated every render, so its numbers win — but it knows nothing
    /// about the model-scoped limit, which must survive the merge.
    #[test]
    fn stdin_rate_limits_override_the_cache_and_keep_the_scoped_window() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let s = session(
            r#"{"model":{"display_name":"Opus 5"},
                "rate_limits":{"five_hour":{"used_percentage":2,"resets_at":1786284000},
                               "seven_day":{"used_percentage":0,"resets_at":1786284000}}}"#,
        );
        assert_eq!(
            render_at(Some(&snap), Some(&s), plain(), NOW).unwrap(),
            "◈ Opus 5 │ S/2% ↺2h00  W/0% ↺2h00  @F/100% ↺3d"
        );
    }

    /// A cache fully superseded by stdin has nothing stale left in it.
    #[test]
    fn stdin_numbers_clear_the_stale_marker_for_the_windows_they_cover() {
        let superseded = r#"{"cachedUsageUtilization":{"fetchedAtMs":1000,"utilization":{"limits":[
            {"kind":"session","percent":4,"severity":"normal","resets_at":"2026-08-09T14:11:00Z"},
            {"kind":"weekly_all","percent":77,"severity":"warning","resets_at":"2026-08-12T12:00:00Z"}
        ]}}}"#;
        let old = parse(superseded, 1000 + crate::limits::STALE_AFTER_MS + 1).unwrap();
        assert!(render_at(Some(&old), None, plain(), NOW).unwrap().ends_with('?'));

        let s = session(
            r#"{"rate_limits":{"five_hour":{"used_percentage":2,"resets_at":1786284000},
                               "seven_day":{"used_percentage":9,"resets_at":1786284000}}}"#,
        );
        assert_eq!(
            render_at(Some(&old), Some(&s), plain(), NOW).unwrap(),
            "◈ S/2% ↺2h00  W/9% ↺2h00"
        );
    }

    /// stdin covers S and W and nothing else, so a model-scoped limit sitting in a
    /// six-hour-old cache is still six hours old.
    #[test]
    fn stdin_does_not_vouch_for_the_limit_it_knows_nothing_about() {
        let old = parse(FIXTURE, 1000 + crate::limits::STALE_AFTER_MS + 1).unwrap();
        let s = session(
            r#"{"rate_limits":{"five_hour":{"used_percentage":2,"resets_at":1786284000},
                               "seven_day":{"used_percentage":9,"resets_at":1786284000}}}"#,
        );
        let out = render_at(Some(&old), Some(&s), plain(), NOW).unwrap();
        assert!(out.ends_with('?'), "the scoped window is still stale: {out}");

        let coloured = render_at(Some(&old), Some(&s), lit(), NOW).unwrap();
        assert!(coloured.contains("\x1b[38;5;244m@F"), "stale scoped window is grey: {coloured:?}");
        assert!(!coloured.contains("\x1b[38;5;167m"), "never red on stale data: {coloured:?}");
        assert!(coloured.contains("\x1b[38;5;71mS"), "fresh windows keep their colour");
    }

    /// Punctuation never takes a severity colour: the slash, the clock and the divider
    /// are structure, and colour is reserved for saying which window needs you.
    #[test]
    fn only_labels_and_numbers_are_coloured() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let s = session(
            r#"{"model":{"display_name":"Opus 5"},"context_window":{"used_percentage":37}}"#,
        );
        let out = render_at(Some(&snap), Some(&s), lit(), NOW).unwrap();
        // The warning window: label and number amber, slash and clock grey.
        assert!(
            out.contains("\x1b[38;5;179mW\x1b[0m\x1b[38;5;244m/\x1b[0m\x1b[38;5;179m77%"),
            "{out:?}"
        );
        assert!(out.contains("\x1b[38;5;244m ↺3d\x1b[0m"), "clock stays dim: {out:?}");
        assert!(out.contains("\x1b[38;5;244m │\x1b[0m"), "divider stays dim: {out:?}");
    }

    /// A window that has already reset shows no number and no countdown.
    #[test]
    fn a_window_that_has_reset_shows_neither_number_nor_clock() {
        let raw = r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
            {"kind":"session","percent":78,"severity":"warning","resets_at":"2026-08-09T07:00:00Z"},
            {"kind":"weekly_all","percent":50,"severity":"warning","resets_at":"2026-08-12T12:00:00Z"}
        ]}}}"#;
        let snap = parse(raw, 1).unwrap();
        assert_eq!(render_at(Some(&snap), None, plain(), NOW).unwrap(), "◈ S/—  W/50% ↺3d");
    }

    /// `resets_at` comes back null on the scoped window in the wild. No clock, no
    /// placeholder, and above all no borrowing the weekly's.
    #[test]
    fn a_window_with_no_reset_time_simply_has_no_clock() {
        let raw = r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
            {"kind":"session","percent":10,"severity":"normal","resets_at":"2026-08-09T13:23:00Z"},
            {"kind":"weekly_scoped","percent":0,"severity":"normal","resets_at":null,
             "scope":{"model":{"display_name":"Fable"}}}
        ]}}}"#;
        let snap = parse(raw, 1).unwrap();
        assert_eq!(render_at(Some(&snap), None, plain(), NOW).unwrap(), "◈ S/10% ↺1h23  @F/0%");
    }

    #[test]
    fn countdowns_pick_their_unit_by_magnitude() {
        assert_eq!(until("2026-08-09T12:18:00Z", NOW).as_deref(), Some("18m"));
        assert_eq!(until("2026-08-09T14:11:00Z", NOW).as_deref(), Some("2h11"));
        assert_eq!(until("2026-08-15T12:00:00Z", NOW).as_deref(), Some("6d"));
        // Already passed, and a countdown never runs backwards.
        assert_eq!(until("2026-08-09T11:59:00Z", NOW), None);
        assert_eq!(until("not a timestamp", NOW), None);
    }

    #[test]
    fn the_bar_is_off_unless_asked_for() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let plainly = render_at(Some(&snap), None, plain(), NOW).unwrap();
        assert!(!plainly.contains('▁'), "{plainly}");

        let barred = Style { bar: true, ..Style::default() };
        assert_eq!(
            render_at(Some(&snap), None, barred, NOW).unwrap(),
            "◈ S/4%▁ ↺2h11  W/77%▇ ↺3d  @F/100%█ ↺3d"
        );
    }

    /// The arrow is news, not an alarm: grey, last, and after the staleness mark
    /// so neither end-marker moves when the other appears.
    #[test]
    fn an_available_update_adds_one_arrow_at_the_end() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let up = Style { update: true, ..Style::default() };
        assert_eq!(
            render_at(Some(&snap), None, up, NOW).unwrap(),
            "◈ S/4% ↺2h11  W/77% ↺3d  @F/100% ↺3d ↑"
        );

        // Stale as well: the `?` keeps its place and the arrow follows it.
        let old = parse(FIXTURE, 1000 + crate::limits::STALE_AFTER_MS + 1).unwrap();
        assert!(render_at(Some(&old), None, up, NOW).unwrap().ends_with("? ↑"));

        let coloured = Style { colour: true, update: true, ..Style::default() };
        let out = render_at(Some(&snap), None, coloured, NOW).unwrap();
        // Dim, last, and an OSC 8 hyperlink around the glyph alone — the leading
        // space stays outside so only the arrow is a click target.
        assert!(
            out.ends_with(concat!(
                " \x1b]8;;https://github.com/MarioPayan/AImeter/releases/latest\x07",
                "\x1b[38;5;244m↑\x1b[0m",
                "\x1b]8;;\x07"
            )),
            "{out:?}"
        );
    }

    /// A terminal without hyperlink support shows the text and ignores the rest,
    /// so the arrow has to carry its meaning without the link. With NO_COLOR there
    /// is no escape sequence at all.
    #[test]
    fn the_arrow_still_reads_without_escapes() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let out =
            render_at(Some(&snap), None, Style { update: true, ..Style::default() }, NOW).unwrap();
        assert!(out.ends_with(" ↑"), "{out:?}");
        assert!(!out.contains('\x1b'), "no escapes at all: {out:?}");
    }

    #[test]
    fn no_update_means_no_arrow() {
        let snap = parse(FIXTURE, 1000).unwrap();
        assert!(!render_at(Some(&snap), None, plain(), NOW).unwrap().contains('↑'));
    }

    #[test]
    fn fill_spans_the_range() {
        assert_eq!(fill(0.0, false), "▁");
        assert_eq!(fill(4.0, false), "▁");
        assert_eq!(fill(50.0, false), "▅");
        assert_eq!(fill(77.0, false), "▇");
        assert_eq!(fill(100.0, false), "█");
        // The ASCII ramp rises in visual density the same way.
        assert_eq!(fill(0.0, true), ".");
        assert_eq!(fill(77.0, true), "#");
        assert_eq!(fill(100.0, true), "%");
    }

    /// The whole segment in ASCII: every glyph replaced, nothing outside 0x7F.
    #[test]
    fn ascii_mode_emits_no_byte_above_ascii() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let s = session(
            r#"{"model":{"display_name":"Opus 5 (1M context)"},
                "effort":{"level":"xhigh"},
                "context_window":{"used_percentage":37}}"#,
        );
        let st = Style { ascii: true, bar: true, update: true, ..Style::default() };
        let out = render_at(Some(&snap), Some(&s), st, NOW).unwrap();
        assert_eq!(out, "* Opus 5.X . 37% | S/4%. ~2h11  W/77%# ~3d  @F/100%% ~3d ^");
        assert!(out.is_ascii(), "{out:?}");

        // An expired window's dash is ASCII too.
        let reset = r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
            {"kind":"session","percent":78,"severity":"warning","resets_at":"2026-08-09T07:00:00Z"}
        ]}}}"#;
        let snap = parse(reset, 1).unwrap();
        let out =
            render_at(Some(&snap), None, Style { ascii: true, ..Style::default() }, NOW).unwrap();
        assert_eq!(out, "* S/-");
    }

    /// The locale check follows POSIX precedence, and no locale at all is the C
    /// locale — which is ASCII, not a shrug.
    #[test]
    fn locale_detection_prefers_lc_all_and_defaults_to_ascii() {
        assert!(locale_is_utf8(None, None, Some("en_US.UTF-8")));
        assert!(locale_is_utf8(None, Some("C.utf8"), None));
        assert!(locale_is_utf8(Some("es_CO.UTF-8"), None, Some("C")));
        // LC_ALL wins even when a lower var says UTF-8.
        assert!(!locale_is_utf8(Some("C"), None, Some("en_US.UTF-8")));
        assert!(!locale_is_utf8(None, None, Some("POSIX")));
        assert!(!locale_is_utf8(None, None, None));
        // Empty is unset, not "set to nothing".
        assert!(locale_is_utf8(Some(""), None, Some("en_US.UTF-8")));
    }

    #[test]
    fn nothing_at_all_prints_nothing() {
        assert!(render_at(None, None, plain(), NOW).is_none());
        assert!(render_at(None, Some(&Session::default()), plain(), NOW).is_none());
    }

    /// With no cache at all — no credentials, first run — stdin alone is enough.
    #[test]
    fn stdin_alone_renders_without_any_cache() {
        let s = session(
            r#"{"model":{"display_name":"Opus 5 (1M context)"},
                "effort":{"level":"high"},
                "context_window":{"used_percentage":8},
                "rate_limits":{"five_hour":{"used_percentage":95,"resets_at":1786284000},
                               "seven_day":{"used_percentage":10,"resets_at":1786284000}}}"#,
        );
        assert_eq!(
            render_at(None, Some(&s), plain(), NOW).unwrap(),
            "◈ Opus 5·H · 8% │ S/95% ↺2h00  W/10% ↺2h00"
        );
        // No severity arrives on stdin, so it is derived: 95% must read as critical.
        let coloured = render_at(None, Some(&s), lit(), NOW).unwrap();
        assert!(coloured.contains("\x1b[38;5;167mS"), "{coloured:?}");
        assert!(coloured.contains("\x1b[38;5;71mW"), "10% stays green: {coloured:?}");
    }

    #[test]
    fn percent_is_rounded_not_truncated() {
        let raw = r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
            {"kind":"session","percent":4.6,"severity":"normal"}]}}}"#;
        let snap = parse(raw, 1).unwrap();
        assert_eq!(render_at(Some(&snap), None, plain(), NOW).unwrap(), "◈ S/5%");
    }

    /// Stale data must be visibly stale even where colour is unavailable.
    #[test]
    fn stale_is_marked_without_relying_on_colour() {
        let old = parse(FIXTURE, 1000 + crate::limits::STALE_AFTER_MS + 1).unwrap();
        assert!(render_at(Some(&old), None, plain(), NOW).unwrap().ends_with('?'));
        let coloured = render_at(Some(&old), None, lit(), NOW).unwrap();
        assert!(coloured.ends_with("?\x1b[0m"));
        assert!(!coloured.contains("\x1b[38;5;167m"), "stale never shows red: {coloured:?}");
    }
}
