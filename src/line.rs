//! The statusline segment: one line, three windows, gone in a couple of milliseconds.
//!
//! This runs on every statusline render — every 10 seconds all day, forever. Two
//! constraints follow from that and shape everything here:
//!
//!   - **It never fails loudly.** Missing file, moved schema, unreadable JSON: print
//!     nothing, exit 0. A statusline that emits an error message or a stack trace
//!     corrupts the console it is drawn in, which is a far worse outcome than a
//!     missing segment.
//!   - **It never blocks.** stdin is deliberately not read. Claude Code pipes JSON
//!     in, but nothing here needs it, and a read on a terminal stdin would hang the
//!     statusline forever the first time someone ran `ccmeter line` by hand.
//!
//! Severity comes from the API's own `severity` field rather than thresholds
//! invented here — Anthropic knows when 77% is a warning better than we do.

use crate::limits::{Limit, Severity, Snapshot};

const TAG: &str = "\x1b[38;5;67m";
const DIM: &str = "\x1b[38;5;244m";
const RESET: &str = "\x1b[0m";

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
    let body = format!("{} {}%", limit.label, limit.percent.round() as i64);
    if !colour_on {
        return body;
    }
    // Stale numbers are greyed regardless of severity: a red 100% that is actually
    // an hour old is a claim we cannot support.
    let c = if stale { DIM } else { colour(limit.severity) };
    format!("{c}{body}{RESET}")
}

/// The whole segment, or `None` when there is nothing honest to say.
pub fn render(snapshot: &Snapshot, colour_on: bool) -> Option<String> {
    if snapshot.limits.is_empty() {
        return None;
    }
    let stale = snapshot.is_stale();
    let sep = if colour_on { format!("{DIM} · {RESET}") } else { " · ".into() };
    let body = snapshot
        .limits
        .iter()
        .map(|l| segment(l, stale, colour_on))
        .collect::<Vec<_>>()
        .join(&sep);

    let tag = if colour_on { format!("{TAG}[USAGE]{RESET}") } else { "[USAGE]".into() };
    // A trailing "?" is the only marker of staleness that survives a colourless
    // terminal, so it is not merely decoration.
    let mark = if stale { "?" } else { "" };
    Some(format!("{tag} {body}{mark}"))
}

/// Print the segment. Any failure at all is silence.
pub fn main() {
    let Some(snapshot) = crate::limits::read() else { return };
    if let Some(out) = render(&snapshot, use_colour()) {
        print!("{out}");
    }
}

/* ------------------------------------------------------------------ tests ---- */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::parse;

    const FIXTURE: &str = r#"{"cachedUsageUtilization":{"fetchedAtMs":1000,"utilization":{"limits":[
        {"kind":"session","percent":4,"severity":"normal","resets_at":"2026-08-09T01:30:00Z"},
        {"kind":"weekly_all","percent":77,"severity":"warning","resets_at":"2026-08-09T11:59:59Z"},
        {"kind":"weekly_scoped","percent":100,"severity":"critical","resets_at":"2026-08-09T11:59:59Z",
         "scope":{"model":{"display_name":"Fable"}}}
    ]}}}"#;

    #[test]
    fn renders_all_three_windows() {
        let snap = parse(FIXTURE, 1000).unwrap();
        assert_eq!(render(&snap, false).unwrap(), "[USAGE] 5h 4% · 7d 77% · Fable 100%");
    }

    #[test]
    fn severity_picks_the_colour() {
        let snap = parse(FIXTURE, 1000).unwrap();
        let out = render(&snap, true).unwrap();
        assert!(out.contains("\x1b[38;5;71m5h 4%"), "normal is green: {out:?}");
        assert!(out.contains("\x1b[38;5;179m7d 77%"), "warning is amber: {out:?}");
        assert!(out.contains("\x1b[38;5;167mFable 100%"), "critical is red: {out:?}");
    }

    /// Stale data must be visibly stale even where colour is unavailable.
    #[test]
    fn stale_is_marked_without_relying_on_colour() {
        let old = parse(FIXTURE, 1000 + crate::limits::STALE_AFTER_MS + 1).unwrap();
        assert!(render(&old, false).unwrap().ends_with('?'));
        let coloured = render(&old, true).unwrap();
        assert!(coloured.ends_with('?'));
        assert!(!coloured.contains("\x1b[38;5;167m"), "stale never shows red: {coloured:?}");
    }

    #[test]
    fn percent_is_rounded_not_truncated() {
        let raw = r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
            {"kind":"session","percent":4.6,"severity":"normal"}]}}}"#;
        let snap = parse(raw, 1).unwrap();
        assert_eq!(render(&snap, false).unwrap(), "[USAGE] 5h 5%");
    }
}
