//! Claude Code's own view of your rate limits, read from the cache it keeps.
//!
//! `~/.claude.json` holds `cachedUsageUtilization`, refreshed by the CLI while a
//! session runs. Two decisions about how it is parsed are worth stating, because
//! both are about surviving a schema that is not ours:
//!
//!   - Only `utilization.limits[]` is read. Its siblings (`five_hour`, `seven_day`,
//!     and a rotating cast of codenames — `nimbus_quill`, `iguana_necktie`,
//!     `omelette_promotional`) say the same thing in a shape that changes. The
//!     array is self-describing: every entry carries its own kind, group and scope.
//!   - Every field is `#[serde(default)]`. A limit that gains a field, loses one,
//!     or turns a number into null should cost us that one limit, not the segment.
//!
//! It is a cache, not a feed: `fetched_at_ms` only advances while Claude Code is
//! running. `Snapshot::age` exists so callers can say so rather than presenting a
//! stale number as current.

use serde::Deserialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// How old the cache may be before a reader should stop trusting it.
///
/// Six hours, because that is the first point at which the number is certainly
/// wrong rather than merely old: the shortest window Anthropic reports is five
/// hours, so past six the session limit has definitely rolled over.
///
/// Measured rather than assumed — `fetchedAtMs` sat unchanged for thirty minutes
/// of continuous API traffic, so Claude Code refreshes this on its own slow
/// cadence. A tighter threshold would grey the segment out almost permanently and
/// throw away the severity colours, which are the only reason to look at it.
pub const STALE_AFTER_MS: i64 = 6 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

impl Severity {
    fn parse(s: &str) -> Self {
        match s {
            "critical" => Severity::Critical,
            "warning" => Severity::Warning,
            _ => Severity::Normal,
        }
    }

    /// Our own thresholds, used only for numbers that arrive without a severity.
    ///
    /// The statusline's stdin payload carries `used_percentage` and nothing about
    /// how alarmed to be, so this is a judgement the API normally makes for us.
    /// Calibrated against what it actually returned: 0–4% normal, 78% warning,
    /// 100% critical.
    pub fn from_percent(percent: f64) -> Self {
        match percent {
            p if p >= 90.0 => Severity::Critical,
            p if p >= 50.0 => Severity::Warning,
            _ => Severity::Normal,
        }
    }
}

/// One limit window, flattened into just what a display needs.
#[derive(Debug, Clone)]
pub struct Limit {
    /// One-letter display label: `S` session, `W` week, or `@` plus the scoped
    /// model's initial (`@F`). The `@` is what keeps a Sonnet-scoped `@S` from
    /// being read as the session's `S`.
    pub label: String,
    pub percent: f64,
    pub severity: Severity,
    /// RFC 3339, as the API sends it. `None` when the window has no reset.
    pub resets_at: Option<String>,
}

impl Limit {
    /// The window this percentage describes has already rolled over.
    ///
    /// Worth its own concept rather than folding into staleness: an old reading of
    /// a *current* window is still roughly right, but once `resets_at` passes, the
    /// counter went back to zero and the number is not old — it is wrong. Observed
    /// in the wild at 78% for a weekly window that had reset six hours earlier.
    pub fn expired_at(&self, now: i64) -> bool {
        self.resets_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .is_some_and(|when| when.timestamp() <= now)
    }
}

/// What one read of the cache saw.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub limits: Vec<Limit>,
    /// Milliseconds since the CLI last refreshed the cache, when it said.
    pub age_ms: Option<i64>,
}

impl Snapshot {
    pub fn is_stale(&self) -> bool {
        self.age_ms.map(|age| age > STALE_AFTER_MS).unwrap_or(true)
    }
}

/* ------------------------------------------------------------ wire shapes ---- */

#[derive(Deserialize, Default)]
struct Root {
    #[serde(default, rename = "cachedUsageUtilization")]
    cached_usage_utilization: Option<Cached>,
}

#[derive(Deserialize, Default)]
struct Cached {
    #[serde(default, rename = "fetchedAtMs")]
    fetched_at_ms: Option<i64>,
    #[serde(default)]
    utilization: Option<Utilization>,
}

#[derive(Deserialize, Default)]
struct Utilization {
    #[serde(default)]
    limits: Vec<WireLimit>,
}

#[derive(Deserialize, Default)]
struct WireLimit {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    percent: Option<f64>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(default)]
    scope: Option<Scope>,
}

#[derive(Deserialize, Default)]
struct Scope {
    #[serde(default)]
    model: Option<ScopeModel>,
}

#[derive(Deserialize, Default)]
struct ScopeModel {
    #[serde(default)]
    display_name: Option<String>,
}

/* ------------------------------------------------------------------ read ---- */

pub fn claude_json_path() -> PathBuf {
    home().join(".claude.json")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The freshest limits available, from whichever source has them.
///
/// Two copies exist: ours, fetched straight from the usage endpoint, and Claude
/// Code's, refreshed on its own schedule. Whichever was fetched more recently
/// wins — so this keeps working unchanged if the endpoint stops answering, and
/// needs no flag to say which one is in play.
pub fn read() -> Option<Snapshot> {
    let ours = crate::fetch::read_cached();
    let theirs = read_from(&claude_json_path());
    match (ours, theirs) {
        (Some(a), Some(b)) => {
            Some(if a.age_ms.unwrap_or(i64::MAX) <= b.age_ms.unwrap_or(i64::MAX) { a } else { b })
        }
        (a, b) => a.or(b),
    }
}

/// Read and flatten one file. `None` when it is missing, unreadable, not JSON, or
/// carries no limits — every one of which is "say nothing", not "crash".
pub fn read_from(path: &std::path::Path) -> Option<Snapshot> {
    let raw = std::fs::read_to_string(path).ok()?;
    parse(&raw, now_ms())
}

/// Split out from `read_from` so tests can pin "now" and feed a fixture.
pub fn parse(raw: &str, now_ms: i64) -> Option<Snapshot> {
    let root: Root = serde_json::from_str(raw).ok()?;
    let cached = root.cached_usage_utilization?;
    let limits: Vec<Limit> = cached
        .utilization
        .map(|u| u.limits)
        .unwrap_or_default()
        .into_iter()
        .filter_map(flatten)
        .collect();

    if limits.is_empty() {
        return None;
    }
    Some(Snapshot { limits, age_ms: cached.fetched_at_ms.map(|t| now_ms - t) })
}

/// A limit with no percent tells us nothing, so it is dropped rather than shown
/// as zero — "0%" and "unknown" are very different claims to make about a ceiling.
fn flatten(w: WireLimit) -> Option<Limit> {
    let percent = w.percent?;
    let scoped_model =
        w.scope.and_then(|s| s.model).and_then(|m| m.display_name).filter(|n| !n.is_empty());

    // One letter each, because the segment shares a line and these three windows are
    // always the same three things. The scoped one takes `@` plus its model's initial:
    // the mark is what distinguishes it, not the letter, so a Sonnet-scoped limit reads
    // `@S` and never collides with the session's `S`.
    let label = match (scoped_model, w.kind.as_str()) {
        (Some(model), _) => {
            format!("@{}", model.chars().next().unwrap_or('?').to_uppercase())
        }
        (None, "session") => "S".into(),
        (None, "weekly_all") => "W".into(),
        (None, "weekly_scoped") => "@".into(),
        (None, "") => "?".into(),
        (None, other) => other.chars().next().unwrap_or('?').to_uppercase().to_string(),
    };

    Some(Limit {
        label,
        percent,
        severity: Severity::parse(w.severity.as_deref().unwrap_or("normal")),
        resets_at: w.resets_at,
    })
}

/* ------------------------------------------------------------------ tests ---- */

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `~/.claude.json`, including the codename keys we ignore.
    const FIXTURE: &str = r#"{
      "numStartups": 1,
      "cachedUsageUtilization": {
        "fetchedAtMs": 1000,
        "utilization": {
          "five_hour": { "utilization": 4 },
          "nimbus_quill": { "utilization": 0, "resets_at": null },
          "iguana_necktie": null,
          "limits": [
            { "kind": "session", "group": "session", "percent": 4,
              "severity": "normal", "resets_at": "2026-08-09T01:30:00.728724+00:00",
              "scope": null, "is_active": false },
            { "kind": "weekly_all", "group": "weekly", "percent": 77,
              "severity": "warning", "resets_at": "2026-08-09T11:59:59.728746+00:00",
              "scope": null, "is_active": false },
            { "kind": "weekly_scoped", "group": "weekly", "percent": 100,
              "severity": "critical", "resets_at": "2026-08-09T11:59:59.728920+00:00",
              "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null },
              "is_active": true }
          ]
        }
      }
    }"#;

    #[test]
    fn reads_the_three_windows_and_marks_the_scoped_one() {
        let snap = parse(FIXTURE, 1000).expect("fixture parses");
        let labels: Vec<&str> = snap.limits.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["S", "W", "@F"]);
        assert_eq!(snap.limits[2].severity, Severity::Critical);
        assert_eq!(snap.limits[2].percent, 100.0);
    }

    /// The scoped label is `@` plus an initial, and the `@` is what disambiguates —
    /// so a Sonnet-scoped limit is `@S` and cannot be read as the session's `S`.
    /// A lowercase or missing display name must not produce a lowercase label.
    #[test]
    fn the_scoped_label_is_always_at_plus_a_capital() {
        let scoped = |name: &str| {
            let raw = format!(
                r#"{{"cachedUsageUtilization":{{"fetchedAtMs":1,"utilization":{{"limits":[
                    {{"kind":"weekly_scoped","percent":1,"scope":{{"model":{{"display_name":"{name}"}}}}}}
                ]}}}}}}"#
            );
            parse(&raw, 1).unwrap().limits[0].label.clone()
        };
        assert_eq!(scoped("Fable"), "@F");
        assert_eq!(scoped("Sonnet"), "@S");
        assert_eq!(scoped("opus"), "@O");
        // No model name at all: the mark alone still says "scoped to a model".
        let bare = r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
            {"kind":"weekly_scoped","percent":1}]}}}"#;
        assert_eq!(parse(bare, 1).unwrap().limits[0].label, "@");
    }

    #[test]
    fn staleness_is_measured_from_fetched_at() {
        assert!(!parse(FIXTURE, 1000).unwrap().is_stale());
        // Half an hour old is normal — Claude Code refreshes on its own cadence.
        assert!(!parse(FIXTURE, 1000 + 30 * 60 * 1000).unwrap().is_stale());
        // Past the shortest reported window, the number is wrong, not just old.
        assert!(parse(FIXTURE, 1000 + STALE_AFTER_MS + 1).unwrap().is_stale());
    }

    /// The whole point of `#[serde(default)]`: a limit that loses `severity` and
    /// gains a field it never had must not take the other two down with it.
    #[test]
    fn survives_a_schema_that_moved() {
        let raw = r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
            {"kind":"session","percent":5,"brand_new_field":{"nested":true}},
            {"kind":"weekly_all","severity":"warning"},
            {"kind":"weekly_all","percent":80,"severity":"warning"}
        ]}}}"#;
        let snap = parse(raw, 1).expect("still parses");
        // The middle entry has no percent, so it is dropped, not shown as 0%.
        assert_eq!(snap.limits.len(), 2);
        assert_eq!(snap.limits[0].severity, Severity::Normal);
        assert_eq!(snap.limits[1].percent, 80.0);
    }

    #[test]
    fn nothing_to_say_is_none_not_an_empty_snapshot() {
        assert!(parse("{}", 0).is_none());
        assert!(parse("not json at all", 0).is_none());
        assert!(parse(r#"{"cachedUsageUtilization":{"utilization":{"limits":[]}}}"#, 0).is_none());
    }
}
