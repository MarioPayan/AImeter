//! Codex's view of your ChatGPT rate limits — the second tool on the same line.
//!
//! The same two kinds of place the Claude numbers come from, ranked the same way:
//!
//!   - **Codex's own session log.** Every turn appends a `token_count` event to the
//!     newest `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, carrying the limits as
//!     the server last reported them. No credentials, no network — and only as
//!     current as your last Codex turn, which for an occasional user is days.
//!   - **`GET /backend-api/wham/usage`**, the endpoint Codex's own `/status` reads.
//!     Current whatever you last ran, and reached with the token in
//!     `~/.codex/auth.json`.
//!
//! Whichever reading is younger wins, so nothing needs a flag to say which is in
//! play, and the endpoint going away costs freshness rather than the group.
//!
//! The constraints are `fetch.rs`'s, for the same reasons. **The token is read,
//! never written**: OpenAI rotates refresh tokens, so refreshing here would log
//! Codex out. It lasts ten days; past that the endpoint answers 401 and the log is
//! all there is until Codex next runs. `AIMETER_NO_FETCH` stops the token being
//! read at all, and `AIMETER_NO_CODEX` removes the group entirely.
//!
//! Neither shape is ours, and the two disagree on names — `primary` against
//! `primary_window`, `resets_at` against `reset_at`, minutes against seconds. One
//! struct reads both through aliases, so there is one place for a label to be
//! decided. That label comes from the window's *length*, never from its slot:
//! `primary` has been observed holding the weekly window with `secondary` null.

use crate::limits::{Limit, Severity, Snapshot};
use serde::Deserialize;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

const URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// How much of the log's end is read. A `token_count` line is under a kilobyte and
/// Codex writes one after every model response, so the last one is almost always
/// in here. The whole file is not an option: single lines of 2 MB have been
/// observed, and this runs inside a render that budgets single-digit milliseconds.
const TAIL: u64 = 64 * 1024;

/* ------------------------------------------------------------ wire shapes ---- */

/// Both sources, through aliases: the log says `primary`, the endpoint
/// `primary_window`.
#[derive(Deserialize, Default)]
struct Windows {
    #[serde(default, alias = "primary_window")]
    primary: Option<Window>,
    #[serde(default, alias = "secondary_window")]
    secondary: Option<Window>,
}

#[derive(Deserialize, Default)]
struct Window {
    #[serde(default)]
    used_percent: Option<f64>,
    /// The log's unit.
    #[serde(default)]
    window_minutes: Option<i64>,
    /// The endpoint's.
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    /// Epoch seconds in both, under two spellings.
    #[serde(default, alias = "reset_at")]
    resets_at: Option<i64>,
}

#[derive(Deserialize, Default)]
struct LogLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    payload: Option<Payload>,
}

#[derive(Deserialize, Default)]
struct Payload {
    #[serde(default)]
    rate_limits: Option<Windows>,
}

/// Our copy of the endpoint's answer, as `store_shape` wrote it.
#[derive(Deserialize, Default)]
struct Stored {
    #[serde(default, rename = "fetchedAtMs")]
    fetched_at_ms: Option<i64>,
    #[serde(default)]
    rate_limit: Option<Windows>,
}

/* ------------------------------------------------------------------ parse ---- */

/// A window with no percent tells us nothing, so it is dropped rather than shown as
/// zero — the same call `limits::flatten` makes.
fn flatten(w: Window) -> Option<Limit> {
    let percent = w.used_percent?;
    // `S` and `W` mean what they mean for Claude — five hours, seven days — and the
    // `cx` in front of the group says whose they are. Any other length is printed
    // as sent: a label guessed from the nearest familiar window would be a claim
    // about the limit that the API did not make.
    let label = match w.window_minutes.or(w.limit_window_seconds.map(|s| s / 60)) {
        Some(300) => "S".into(),
        Some(10_080) => "W".into(),
        Some(m) if m >= 1440 => format!("{}d", m / 1440),
        Some(m) if m >= 60 => format!("{}h", m / 60),
        Some(m) if m > 0 => format!("{m}m"),
        _ => "?".into(),
    };
    Some(Limit {
        label,
        percent,
        // Codex sends a percentage and no opinion, like the stdin payload.
        severity: Severity::from_percent(percent),
        resets_at: w
            .resets_at
            .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0).map(|t| t.to_rfc3339())),
    })
}

fn snapshot(w: Windows, age_ms: Option<i64>) -> Option<Snapshot> {
    let limits: Vec<Limit> =
        [w.primary, w.secondary].into_iter().flatten().filter_map(flatten).collect();
    (!limits.is_empty()).then_some(Snapshot { limits, age_ms })
}

fn parse_stored(raw: &str, now_ms: i64) -> Option<Snapshot> {
    let stored: Stored = serde_json::from_str(raw).ok()?;
    snapshot(stored.rate_limit?, stored.fetched_at_ms.map(|t| now_ms - t))
}

/// The most recent reading in the end of a log, walking backwards.
///
/// The cut lands mid-line more often than not, and that first fragment simply fails
/// to parse. A reading with no timestamp is kept with no age, which `is_stale` reads
/// as old: an undated number is one nobody can vouch for, not one to throw away.
fn parse_log_tail(tail: &str, now_ms: i64) -> Option<Snapshot> {
    tail.lines().rev().filter(|l| l.contains("\"token_count\"")).find_map(|l| {
        let line: LogLine = serde_json::from_str(l).ok()?;
        let age_ms = line
            .timestamp
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|t| now_ms - t.timestamp_millis());
        snapshot(line.payload?.rate_limits?, age_ms)
    })
}

/* ------------------------------------------------------------------- read ---- */

/// `$CODEX_HOME` is Codex's own override for where it keeps everything.
fn dir() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::limits::home().join(".codex"))
}

fn auth_path() -> PathBuf {
    dir().join("auth.json")
}

fn cache_path() -> PathBuf {
    crate::fetch::data_dir().join("codex.json")
}

/// Codex is installed here and nobody has asked for it to be left out.
///
/// Everything in this module sits behind this. On a machine with no Codex it is one
/// `stat` that says no: no log is looked for, no token is read, no request is made,
/// and the segment never grows a `cx`. Most people who install this have no Codex,
/// and for them it has to be exactly the tool it was.
pub fn present() -> bool {
    available(std::env::var("AIMETER_NO_CODEX").ok().as_deref(), &dir())
}

/// Split from `present` so it can be tested without mutating a process-global env
/// var, or depending on what the machine running the tests has installed.
fn available(no_codex: Option<&str>, dir: &Path) -> bool {
    !crate::fetch::off_switch(no_codex) && dir.is_dir()
}

fn greatest(dir: &Path, keep: impl Fn(&str) -> bool) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(&keep))
        .map(|e| e.path())
        .max()
}

/// The newest session log, found by name alone.
///
/// `sessions/YYYY/MM/DD/rollout-<timestamp>-<id>.jsonl` is zero-padded at every
/// level, so the greatest name is the latest — three directory listings and no
/// `stat` of anything.
// ponytail: newest by *start* time. A long-running or resumed session that is still
// being written while a later one sits idle loses to it, and the cost is an older
// reading that says how old it is, never a wrong one. Compare mtimes across the last
// day's files if that ever shows.
fn newest_rollout(sessions: &Path) -> Option<PathBuf> {
    let mut at = sessions.to_path_buf();
    for _ in 0..3 {
        at = greatest(&at, |n| n.bytes().all(|b| b.is_ascii_digit()))?;
    }
    greatest(&at, |n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
}

// ponytail: a tail swallowed whole by one giant line — a 2 MB tool output after the
// last reading — finds nothing, and the endpoint copy or silence covers that render.
// Widen TAIL, or scan back in chunks, if it turns out to happen in practice.
fn read_log() -> Option<Snapshot> {
    let mut file = std::fs::File::open(newest_rollout(&dir().join("sessions"))?).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut tail = Vec::with_capacity(TAIL as usize);
    file.take(TAIL).read_to_end(&mut tail).ok()?;
    parse_log_tail(&String::from_utf8_lossy(&tail), crate::fetch::now_ms())
}

fn read_cached() -> Option<Snapshot> {
    parse_stored(&std::fs::read_to_string(cache_path()).ok()?, crate::fetch::now_ms())
}

/// The freshest Codex limits available, or `None` when there is no Codex here.
///
/// `limits::read` over again: our copy from the endpoint against the tool's own
/// file, younger wins, and the tool's file is not opened at all while ours is inside
/// the refresh interval.
pub fn read() -> Option<Snapshot> {
    if !present() {
        return None;
    }
    let ours = read_cached();
    if ours.as_ref().is_some_and(crate::limits::is_current) {
        return ours;
    }
    crate::limits::fresher(ours, read_log())
}

/* ------------------------------------------------------------------ fetch ---- */

/// Written when the endpoint turns the token down.
fn rejected_path() -> PathBuf {
    crate::fetch::data_dir().join("codex.rejected")
}

/// The token in `auth.json` is the one that was already turned down.
///
/// Claude Code's credentials carry their own expiry, so a dead token there is never
/// sent. Codex's expiry is inside the JWT, and someone who opens Codex twice a month
/// holds a dead one most of the time — without this, every refresh would present it
/// again, once a minute, to an endpoint that is not ours. Codex rewrites the file
/// when it refreshes, and that is the signal to try again.
fn already_rejected() -> bool {
    let modified = |p: PathBuf| std::fs::metadata(p).ok()?.modified().ok();
    match (modified(rejected_path()), modified(auth_path())) {
        (Some(rejected), Some(auth)) => auth <= rejected,
        _ => false,
    }
}

/// The access token Codex is currently using, and the account it belongs to.
/// Read-only, and never logged.
fn credentials() -> Option<(String, Option<String>)> {
    let raw = std::fs::read_to_string(auth_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    // Absent when Codex is signed in with an API key, which has no plan to meter.
    let tokens = value.get("tokens")?;
    let token = tokens.get("access_token")?.as_str().filter(|t| !t.is_empty())?;
    let account = tokens.get("account_id").and_then(|v| v.as_str()).map(String::from);
    Some((token.to_string(), account))
}

/// Fetch once and write the result.
pub fn fetch_now() -> Result<(), String> {
    if crate::fetch::disabled() {
        return Err("AIMETER_NO_FETCH is set — not reading the token".into());
    }
    if already_rejected() {
        return Err("token already rejected — waiting for Codex to refresh it".into());
    }
    let (token, account) = credentials().ok_or("no ChatGPT sign-in in Codex's auth.json")?;

    let mut request = ureq::get(URL)
        .set("authorization", &format!("Bearer {token}"))
        .set("accept", "application/json")
        .set("user-agent", concat!("aimeter/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10));
    if let Some(account) = &account {
        request = request.set("chatgpt-account-id", account);
    }
    let body: serde_json::Value = request
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401, _) => {
                let _ = crate::fetch::write_atomically(&rejected_path(), "");
                "token rejected (401) — Codex will refresh it".into()
            }
            ureq::Error::Status(code, _) => format!("usage endpoint returned {code}"),
            ureq::Error::Transport(t) => format!("cannot reach the usage endpoint: {t}"),
        })?
        .into_json()
        .map_err(|e| format!("usage endpoint sent something that is not JSON: {e}"))?;

    let text = serde_json::to_string(&store_shape(&body, crate::fetch::now_ms()))
        .map_err(|e| e.to_string())?;
    crate::fetch::write_atomically(&cache_path(), &text)
}

/// Only the two windows are carried over. The response also names the account —
/// email, user id, account id — and its credits and spend controls; none of it is
/// displayed, so none of it is written to disk.
fn store_shape(body: &serde_json::Value, now: i64) -> serde_json::Value {
    let window = |name: &str| {
        body.get("rate_limit").and_then(|r| r.get(name)).cloned().unwrap_or(serde_json::Value::Null)
    };
    serde_json::json!({
        "fetchedAtMs": now,
        "rate_limit": {
            "primary_window": window("primary_window"),
            "secondary_window": window("secondary_window"),
        }
    })
}

/* ------------------------------------------------------------------ tests ---- */

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-08-09T12:00:00Z, in milliseconds — the same instant `line.rs` pins.
    const NOW_MS: i64 = 1_786_276_800_000;

    /// What the endpoint returns, including the parts we refuse to keep. The weekly
    /// window sits in `primary` and `secondary` is null, exactly as observed.
    fn response() -> serde_json::Value {
        serde_json::json!({
            "user_id": "user-abc", "account_id": "acct-abc", "email": "someone@example.com",
            "plan_type": "prolite",
            "rate_limit": {
                "allowed": true, "limit_reached": false,
                "primary_window": { "used_percent": 33, "limit_window_seconds": 604_800,
                                    "reset_after_seconds": 54_398, "reset_at": 1_786_536_000 },
                "secondary_window": null
            },
            "credits": { "has_credits": false, "balance": "0" },
            "spend_control": { "reached": false }
        })
    }

    /// One `token_count` line as Codex writes it, an hour before `NOW_MS`.
    fn log_line(percent: f64, at: &str) -> String {
        serde_json::json!({
            "timestamp": at, "type": "event_msg",
            "payload": { "type": "token_count", "info": null, "rate_limits": {
                "limit_id": "codex",
                "primary": { "used_percent": percent, "window_minutes": 10_080, "resets_at": 1_786_536_000 },
                "secondary": null, "plan_type": "prolite" } }
        })
        .to_string()
    }

    #[test]
    fn the_log_and_the_endpoint_describe_the_same_limit() {
        let stored = store_shape(&response(), NOW_MS).to_string();
        let ours = parse_stored(&stored, NOW_MS).expect("our cache parses");
        let theirs = parse_log_tail(&log_line(33.0, "2026-08-09T11:00:00.000Z"), NOW_MS)
            .expect("the log parses");
        for snap in [&ours, &theirs] {
            assert_eq!(snap.limits.len(), 1);
            assert_eq!(snap.limits[0].label, "W", "weekly, though it arrived in `primary`");
            assert_eq!(snap.limits[0].percent, 33.0);
            assert_eq!(snap.limits[0].resets_at.as_deref(), Some("2026-08-12T12:00:00+00:00"));
        }
        assert_eq!(ours.age_ms, Some(0));
        assert_eq!(theirs.age_ms, Some(3_600_000), "aged from the event, not from the read");
    }

    #[test]
    fn the_label_follows_the_window_length_not_the_slot() {
        let label = |json: serde_json::Value| {
            flatten(serde_json::from_value(json).unwrap()).map(|l| l.label)
        };
        let of = |minutes: i64| {
            label(serde_json::json!({ "used_percent": 1, "window_minutes": minutes }))
        };
        assert_eq!(of(300).as_deref(), Some("S"));
        assert_eq!(of(10_080).as_deref(), Some("W"));
        // Lengths nobody has named are printed as sent, not rounded to a familiar one.
        assert_eq!(of(4_320).as_deref(), Some("3d"));
        assert_eq!(of(180).as_deref(), Some("3h"));
        assert_eq!(of(30).as_deref(), Some("30m"));
        assert_eq!(label(serde_json::json!({ "used_percent": 1 })).as_deref(), Some("?"));
        assert_eq!(
            label(serde_json::json!({ "used_percent": 1, "limit_window_seconds": 18_000 }))
                .as_deref(),
            Some("S"),
            "the endpoint counts in seconds"
        );
    }

    #[test]
    fn severity_is_derived_because_codex_sends_none() {
        let at = |p: f64| {
            flatten(serde_json::from_value(serde_json::json!({ "used_percent": p })).unwrap())
                .unwrap()
                .severity
        };
        assert_eq!(at(33.0), Severity::Normal);
        assert_eq!(at(78.0), Severity::Warning);
        assert_eq!(at(100.0), Severity::Critical);
    }

    #[test]
    fn a_window_without_a_percent_is_dropped_not_zeroed() {
        let raw = r#"{"fetchedAtMs":1,"rate_limit":{"primary_window":{"limit_window_seconds":18000},
                      "secondary_window":{"used_percent":5,"limit_window_seconds":604800}}}"#;
        let snap = parse_stored(raw, NOW_MS).unwrap();
        assert_eq!(snap.limits.iter().map(|l| l.label.as_str()).collect::<Vec<_>>(), ["W"]);
        // And nothing at all is `None`, which is what lets the log take over.
        assert!(parse_stored(r#"{"fetchedAtMs":1,"rate_limit":null}"#, NOW_MS).is_none());
        assert!(parse_stored("not json", NOW_MS).is_none());
    }

    #[test]
    fn the_last_reading_in_the_tail_wins() {
        let tail = [
            // The cut lands mid-line: a fragment that even mentions the event.
            r#"nt_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_per"#
                .to_string(),
            log_line(10.0, "2026-08-09T10:00:00.000Z"),
            r#"{"type":"response_item","payload":{"type":"message"}}"#.to_string(),
            log_line(33.0, "2026-08-09T11:00:00.000Z"),
            // A turn that reported no limits does not hide the one before it.
            r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":null}}"#
                .to_string(),
        ]
        .join("\n");
        let snap = parse_log_tail(&tail, NOW_MS).unwrap();
        assert_eq!(snap.limits[0].percent, 33.0);
        assert_eq!(snap.age_ms, Some(3_600_000));
        assert!(parse_log_tail("", NOW_MS).is_none());
    }

    /// Codex signed in with an API key has no plan to meter: it logs the event and
    /// leaves the limits null. That is no reading, so no group — never a `cx W/0%`.
    #[test]
    fn a_log_with_no_limits_in_it_is_no_reading() {
        let tail = [
            r#"{"timestamp":"2026-08-09T11:00:00.000Z","payload":{"type":"token_count","rate_limits":null}}"#,
            r#"{"timestamp":"2026-08-09T11:30:00.000Z","payload":{"type":"token_count","rate_limits":{"primary":null,"secondary":null}}}"#,
        ]
        .join("\n");
        assert!(parse_log_tail(&tail, NOW_MS).is_none());
    }

    /// No Codex directory means no Codex, whatever else is lying around — and the
    /// off switch wins over a directory that is there.
    #[test]
    fn without_a_codex_directory_there_is_no_codex() {
        let here = std::env::temp_dir();
        let nowhere = here.join(format!("aimeter-no-codex-{}", std::process::id()));
        assert!(available(None, &here));
        assert!(!available(None, &nowhere));
        // A file is not an installation.
        let file = here.join(format!("aimeter-codex-file-{}", std::process::id()));
        std::fs::write(&file, "").unwrap();
        let as_file = available(None, &file);
        let _ = std::fs::remove_file(&file);
        assert!(!as_file);

        assert!(!available(Some("1"), &here));
        assert!(available(Some("0"), &here), "`0` is how shells spell not set");
    }

    #[test]
    fn an_undated_reading_is_kept_and_counts_as_stale() {
        let line = r#"{"payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":9,"window_minutes":300}}}}"#;
        let snap = parse_log_tail(line, NOW_MS).unwrap();
        assert_eq!(snap.age_ms, None);
        assert!(snap.is_stale());
    }

    /// The response names the account. None of that is displayed, so none of it
    /// reaches the disk.
    #[test]
    fn only_the_windows_are_persisted() {
        let stored = store_shape(&response(), NOW_MS).to_string();
        for kept_out in [
            "email",
            "example.com",
            "user_id",
            "account_id",
            "acct-abc",
            "credits",
            "spend",
            "plan_type",
        ] {
            assert!(!stored.contains(kept_out), "{kept_out} in {stored}");
        }
        assert!(stored.contains("used_percent"), "the part we do read survives");
    }

    #[test]
    fn the_newest_log_is_found_by_name() {
        let root = std::env::temp_dir().join(format!("aimeter-codex-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for file in [
            "2025/12/31/rollout-2025-12-31T23-59-59-a.jsonl",
            "2026/02/03/rollout-2026-02-03T09-00-00-b.jsonl",
            "2026/02/03/rollout-2026-02-03T18-30-00-c.jsonl",
            "2026/02/03/notes.txt",
            "2026/junk/ignored.jsonl",
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let found = newest_rollout(&root);
        let missing = newest_rollout(&root.join("nowhere"));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            found.as_deref().and_then(Path::file_name).and_then(|n| n.to_str()),
            Some("rollout-2026-02-03T18-30-00-c.jsonl")
        );
        assert!(missing.is_none());
    }
}
