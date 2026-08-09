//! The statusline segment: one line, three windows, gone in a couple of milliseconds.
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
//! Three sources, in order of authority: the stdin payload Claude Code regenerates
//! every render, then our own fetch from the usage endpoint, then Claude Code's
//! cache. stdin is documented, current, and needs no credentials, so it wins on
//! the numbers it carries — which is the 5-hour and 7-day windows and nothing
//! else. The model-scoped limit only exists in the other two.
//!
//! Severity normally comes from the API's own field rather than thresholds
//! invented here — Anthropic knows when 78% is a warning better than we do. The
//! exception is stdin, which sends a percentage and no opinion, so `from_percent`
//! supplies one.

use crate::limits::{Limit, Severity, Snapshot};
use serde::Deserialize;

const TAG: &str = "\x1b[38;5;67m";
const MODEL: &str = "\x1b[38;5;109m";
const DIM: &str = "\x1b[38;5;244m";
const RESET: &str = "\x1b[0m";

/* ----------------------------------------------------------------- session ---- */

/// The JSON Claude Code pipes to a statusline command on every render.
///
/// Only the two fields worth having are declared. `rate_limits` is documented and
/// current — it beats anything cached, and it needs no credentials, which is the
/// whole reason to prefer it. It carries only the 5-hour and 7-day windows, so a
/// model-scoped limit still has to come from the API.
#[derive(Deserialize, Default)]
pub struct Session {
    #[serde(default)]
    model: ModelInfo,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
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
    /// session, not of the model, and this line is shared with two other segments.
    fn model_name(&self) -> Option<&str> {
        let name = self.model.display_name.split(" (").next()?.trim();
        (!name.is_empty()).then_some(name)
    }

    /// Fold the stdin windows over whatever the API or cache had.
    ///
    /// stdin wins on the numbers because it is regenerated every render, but it
    /// sends no severity, so that is derived. Any limit stdin does not know about —
    /// the model-scoped one — is carried through untouched.
    fn merge(&self, snapshot: Option<&Snapshot>) -> Vec<Limit> {
        let mut limits: Vec<Limit> = snapshot.map(|s| s.limits.clone()).unwrap_or_default();
        let Some(fresh) = self.rate_limits.as_ref() else { return limits };

        for (label, window) in [("5h", fresh.five_hour), ("7d", fresh.seven_day)] {
            let Some(window) = window else { continue };
            let resets_at = window.resets_at.and_then(|secs| {
                chrono::DateTime::from_timestamp(secs, 0).map(|t| t.to_rfc3339())
            });
            let updated = Limit {
                label: label.to_string(),
                percent: window.used_percentage,
                severity: Severity::from_percent(window.used_percentage),
                resets_at,
            };
            match limits.iter_mut().find(|l| l.label == label) {
                Some(existing) => *existing = updated,
                None => limits.push(updated),
            }
        }
        limits
    }
}

fn colour(severity: Severity) -> &'static str {
    match severity {
        Severity::Normal => "\x1b[38;5;71m",
        Severity::Warning => "\x1b[38;5;179m",
        Severity::Critical => "\x1b[38;5;167m",
    }
}

/// Honour `NO_COLOR`, and skip escapes when the output is not a terminal — a
/// statusline captured to a log should be readable.
fn use_colour() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

fn segment(limit: &Limit, stale: bool, colour_on: bool) -> String {
    // A window that has already reset gets no number at all. Printing the last
    // reading would be worse than printing nothing: it is not an old measurement
    // of the current window, it is a measurement of a window that no longer exists.
    let expired = limit.expired();
    let body = if expired {
        format!("{} —", limit.label)
    } else {
        format!("{} {}%", limit.label, limit.percent.round() as i64)
    };
    if !colour_on {
        return body;
    }
    // Stale numbers are greyed regardless of severity: a red 100% that is actually
    // an hour old is a claim we cannot support.
    let c = if stale || expired { DIM } else { colour(limit.severity) };
    format!("{c}{body}{RESET}")
}

/// The whole segment, or `None` when there is nothing honest to say.
pub fn render(snapshot: Option<&Snapshot>, session: Option<&Session>, colour_on: bool) -> Option<String> {
    let default_session = Session::default();
    let session = session.unwrap_or(&default_session);
    let limits = session.merge(snapshot);

    // Staleness describes the cached source only. Anything stdin supplied is by
    // definition current, so a payload carrying rate limits clears the marker.
    let stale = snapshot.map(|s| s.is_stale()).unwrap_or(true)
        && session.rate_limits.is_none();

    let model = session.model_name();
    if limits.is_empty() && model.is_none() {
        return None;
    }

    let sep = if colour_on { format!("{DIM} · {RESET}") } else { " · ".into() };
    let mut parts: Vec<String> = Vec::with_capacity(limits.len() + 1);
    if let Some(model) = model {
        parts.push(if colour_on { format!("{MODEL}{model}{RESET}") } else { model.to_string() });
    }
    parts.extend(limits.iter().map(|l| segment(l, stale, colour_on)));

    let tag = if colour_on { format!("{TAG}[USAGE]{RESET}") } else { "[USAGE]".into() };
    // A trailing "?" is the only marker of staleness that survives a colourless
    // terminal, so it is not merely decoration.
    let mark = if stale && !limits.is_empty() { "?" } else { "" };
    Some(format!("{tag} {}{mark}", parts.join(&sep)))
}

/// Print the segment. Any failure at all is silence.
///
/// The refresh is kicked off first and deliberately not waited for: this call
/// prints what is already on disk, and the child it spawned improves what the
/// *next* call prints. That is what keeps the statusline in single-digit
/// milliseconds while the numbers stay a minute old at worst.
pub fn main() {
    // Read the payload first: it is the parent's pipe, and leaving it unread
    // while doing other work is how a statusline ends up blocking its own writer.
    let session = read_session();
    crate::fetch::refresh_in_background();
    if let Some(out) = render(crate::limits::read().as_ref(), session.as_ref(), use_colour()) {
        print!("{out}");
    }
}

/* ------------------------------------------------------------------ tests ---- */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::parse;

    /// Reset times are deliberately far in the future. A fixture dated "tomorrow"
    /// passes until tomorrow, then starts asserting that live windows render as
    /// expired — which is how the two tests below broke once the date rolled over.
    const FIXTURE: &str = r#"{"cachedUsageUtilization":{"fetchedAtMs":1000,"utilization":{"limits":[
        {"kind":"session","percent":4,"severity":"normal","resets_at":"2099-01-01T01:30:00Z"},
        {"kind":"weekly_all","percent":77,"severity":"warning","resets_at":"2099-01-01T11:59:59Z"},
        {"kind":"weekly_scoped","percent":100,"severity":"critical","resets_at":"2099-01-01T11:59:59Z",
         "scope":{"model":{"display_name":"Fable"}}}
    ]}}}"#;

    fn session(json: &str) -> Session {
        serde_json::from_str(json).expect("session parses")
    }

    #[test]
    fn renders_all_three_windows() {
        let snap = parse(FIXTURE, 1000).unwrap();
        assert_eq!(render(Some(&snap), None, false).unwrap(), "[USAGE] 5h 4% · 7d 77% · Fable 100%");
    }

    /// What the question was actually about: the model, from the same payload.
    #[test]
    fn shows_the_model_and_drops_the_parenthetical() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let s = session(r#"{"model":{"id":"claude-opus-5[1m]","display_name":"Opus 5 (1M context)"}}"#);
        assert_eq!(
            render(Some(&snap), Some(&s), false).unwrap(),
            "[USAGE] Opus 5 · 5h 4% · 7d 77% · Fable 100%"
        );
    }

    /// stdin is regenerated every render, so its numbers win — but it knows
    /// nothing about the model-scoped limit, which must survive the merge.
    #[test]
    fn stdin_rate_limits_override_the_cache_and_keep_fable() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let s = session(
            r#"{"model":{"display_name":"Opus 5"},
                "rate_limits":{"five_hour":{"used_percentage":2,"resets_at":4070908800},
                               "seven_day":{"used_percentage":0,"resets_at":4070908800}}}"#,
        );
        assert_eq!(
            render(Some(&snap), Some(&s), false).unwrap(),
            "[USAGE] Opus 5 · 5h 2% · 7d 0% · Fable 100%"
        );
    }

    /// A payload with live rate limits is current by definition, so the staleness
    /// marker must not be inherited from the cache those numbers just replaced.
    #[test]
    fn stdin_numbers_clear_the_stale_marker() {
        let old = parse(FIXTURE, 1000 + crate::limits::STALE_AFTER_MS + 1).unwrap();
        assert!(render(Some(&old), None, false).unwrap().ends_with('?'));

        let s = session(r#"{"rate_limits":{"five_hour":{"used_percentage":2,"resets_at":4070908800}}}"#);
        let out = render(Some(&old), Some(&s), false).unwrap();
        assert!(!out.ends_with('?'), "fresh numbers are not stale: {out}");
        assert!(out.contains("5h 2%"), "{out}");
    }

    /// With no cache at all — no credentials, first run — stdin alone is enough.
    #[test]
    fn stdin_alone_renders_without_any_cache() {
        let s = session(
            r#"{"model":{"display_name":"Opus 5 (1M context)"},
                "rate_limits":{"five_hour":{"used_percentage":95,"resets_at":4070908800},
                               "seven_day":{"used_percentage":10,"resets_at":4070908800}}}"#,
        );
        assert_eq!(render(None, Some(&s), false).unwrap(), "[USAGE] Opus 5 · 5h 95% · 7d 10%");
        // No severity arrives on stdin, so it is derived: 95% must read as critical.
        let coloured = render(None, Some(&s), true).unwrap();
        assert!(coloured.contains("\x1b[38;5;167m5h 95%"), "{coloured:?}");
        assert!(coloured.contains("\x1b[38;5;71m7d 10%"), "10% stays green: {coloured:?}");
    }

    #[test]
    fn nothing_at_all_prints_nothing() {
        assert!(render(None, None, false).is_none());
        assert!(render(None, Some(&Session::default()), false).is_none());
    }

    #[test]
    fn severity_picks_the_colour() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let out = render(Some(&snap), None, true).unwrap();
        assert!(out.contains("\x1b[38;5;71m5h 4%"), "normal is green: {out:?}");
        assert!(out.contains("\x1b[38;5;179m7d 77%"), "warning is amber: {out:?}");
        assert!(out.contains("\x1b[38;5;167mFable 100%"), "critical is red: {out:?}");
    }

    /// Stale data must be visibly stale even where colour is unavailable.
    #[test]
    fn stale_is_marked_without_relying_on_colour() {
        let old = parse(FIXTURE, 1000 + crate::limits::STALE_AFTER_MS + 1).unwrap();
        assert!(render(Some(&old), None, false).unwrap().ends_with('?'));
        let coloured = render(Some(&old), None, true).unwrap();
        assert!(coloured.ends_with('?'));
        assert!(!coloured.contains("\x1b[38;5;167m"), "stale never shows red: {coloured:?}");
    }

    /// The case that prompted this: every window had reset hours earlier, and the
    /// segment was still reporting `7d 78% · Fable 100%` from before the rollover.
    #[test]
    fn a_window_that_has_reset_shows_no_number() {
        let raw = r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
            {"kind":"session","percent":0,"severity":"normal","resets_at":"2026-08-09T07:29:59Z"},
            {"kind":"weekly_all","percent":78,"severity":"warning","resets_at":"2026-08-09T11:59:59Z"},
            {"kind":"weekly_scoped","percent":100,"severity":"critical",
             "resets_at":"2099-01-01T00:00:00Z","scope":{"model":{"display_name":"Fable"}}}
        ]}}}"#;
        let snap = parse(raw, 1).unwrap();
        // The first two have passed; the third has not, so it keeps its number.
        assert_eq!(render(Some(&snap), None, false).unwrap(), "[USAGE] 5h — · 7d — · Fable 100%");

        let coloured = render(Some(&snap), None, true).unwrap();
        assert!(!coloured.contains("\x1b[38;5;179m"), "an expired window is never amber: {coloured:?}");
        assert!(coloured.contains("\x1b[38;5;167mFable 100%"), "the live one keeps its red");
    }

    #[test]
    fn percent_is_rounded_not_truncated() {
        let raw = r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
            {"kind":"session","percent":4.6,"severity":"normal"}]}}}"#;
        let snap = parse(raw, 1).unwrap();
        assert_eq!(render(Some(&snap), None, false).unwrap(), "[USAGE] 5h 5%");
    }
}
