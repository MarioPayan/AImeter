//! Asking Anthropic for the numbers instead of waiting for Claude Code to.
//!
//! `~/.claude.json` is a cache Claude Code refreshes on its own schedule, and that
//! schedule turned out to be useless for a live display: observed 15.5 hours stale,
//! reporting 78% for a weekly window that had reset six hours earlier while the
//! true figure was 0%. `GET /api/oauth/usage` returns exactly the same object,
//! now.
//!
//! Three constraints shape this, and none are negotiable:
//!
//!   - **Credentials are read, never written.** The token belongs to Claude Code.
//!     We do not refresh it, do not touch `~/.claude/.credentials.json`, and on a
//!     401 we fall back to the cache and let Claude Code sort its own token out.
//!     Refreshing it ourselves risks invalidating the one your editor is using.
//!   - **Nothing polls on a timer.** The fetch is triggered by the statusline
//!     already running, so it happens while you work and stops when you stop.
//!     There is no daemon to install and nothing running overnight.
//!   - **The statusline never waits for the network.** The refresh runs in a
//!     detached child; the caller prints whatever it already had.
//!
//!   - **It can be turned off.** `AIMETER_NO_FETCH` stops the token being read at
//!     all. Reading someone's credentials needs an off switch that is not
//!     "uninstall it", and everything still works without this — stdin and the
//!     cache cover every window but the model-scoped one.
//!
//! This uses an endpoint issued to another application. It works today and may
//! not tomorrow — every failure here is silent and falls back to the cache.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// How old our copy may be before a read triggers a background refresh.
pub fn refresh_after() -> Duration {
    parse_refresh(std::env::var("AIMETER_REFRESH_SECS").ok().as_deref())
}

/// Split from `refresh_after` so the parsing can be tested without mutating a
/// process-global env var out from under the tests running beside it.
fn parse_refresh(raw: Option<&str>) -> Duration {
    raw.and_then(|s| s.parse().ok()).map(Duration::from_secs).unwrap_or(Duration::from_secs(60))
}

/// `AIMETER_NO_FETCH` — never read the token, never call the endpoint.
///
/// This exists because reading someone's credentials should have an off switch
/// that is not "uninstall it". A long `AIMETER_REFRESH_SECS` is not that switch:
/// the first run has nothing cached, so it fetches once regardless.
///
/// Everything still works with this set — stdin covers the 5-hour and 7-day
/// windows and Claude Code's cache covers the rest. Only the model-scoped limit
/// gets less current.
pub fn disabled() -> bool {
    off_switch(std::env::var("AIMETER_NO_FETCH").ok().as_deref())
}

fn off_switch(raw: Option<&str>) -> bool {
    matches!(raw.map(str::trim), Some(v) if !v.is_empty() && v != "0")
}

/// Our own directory, not a corner of `~/.claude`. What lives here is derived data
/// we own, it will hold more than one provider's numbers eventually, and writing
/// into another tool's config dir is a collision waiting to happen.
fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::limits::home().join(".local").join("share"))
        .join("aimeter")
}

pub fn cache_path() -> PathBuf {
    data_dir().join("usage.json")
}

/// Touched before every attempt, successful or not.
///
/// The statusline runs every ten seconds; without this, an unreachable network
/// would spawn six doomed children a minute forever. Rate-limiting *attempts*
/// rather than successes is what makes the offline case quiet.
fn attempt_path() -> PathBuf {
    data_dir().join("usage.attempt")
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn age_of(path: &std::path::Path) -> Option<Duration> {
    std::fs::metadata(path).ok()?.modified().ok()?.elapsed().ok()
}

/// The access token Claude Code is currently using. Read-only, and never logged.
fn access_token() -> Option<String> {
    let raw =
        std::fs::read_to_string(crate::limits::home().join(".claude").join(".credentials.json"))
            .ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let oauth = value.get("claudeAiOauth")?;
    // An expired token would just 401; checking first saves a pointless round trip.
    if let Some(expires) = oauth.get("expiresAt").and_then(|v| v.as_i64()) {
        if expires <= now_ms() {
            return None;
        }
    }
    oauth.get("accessToken")?.as_str().map(String::from)
}

/// Fetch once and write the result. Returns the stored JSON on success.
pub fn fetch_now() -> Result<String, String> {
    if disabled() {
        return Err("AIMETER_NO_FETCH is set — not reading the token".into());
    }
    let token = access_token().ok_or("no usable Claude Code token")?;
    let body: serde_json::Value = ureq::get(URL)
        .set("authorization", &format!("Bearer {token}"))
        .set("accept", "application/json")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| match e {
            // Not an error worth shouting about: Claude Code will refresh its own
            // token when it next runs, and we pick the new one up for free.
            ureq::Error::Status(401, _) => {
                "token rejected (401) — Claude Code will refresh it".into()
            }
            ureq::Error::Status(code, _) => format!("usage endpoint returned {code}"),
            ureq::Error::Transport(t) => format!("cannot reach the usage endpoint: {t}"),
        })?
        .into_json()
        .map_err(|e| format!("usage endpoint sent something that is not JSON: {e}"))?;

    let text = serde_json::to_string(&store_shape(&body, now_ms())).map_err(|e| e.to_string())?;

    let path = cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // The temp name carries the pid. Two `aimeter line` renders can clear the
    // attempt-file check in the same millisecond and spawn two fetches; a shared
    // temp path lets them interleave their writes into one file that both then
    // rename. The rename stays atomic, so a pid is the whole fix.
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, &text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(text)
}

/// Wrap the response in exactly the shape Claude Code uses, so `limits::parse`
/// serves both files and neither source needs its own reader.
///
/// Only `limits[]` is carried over. The response also holds `spend`, `extra_usage`
/// and a rotating cast of codenamed windows; none of it is read, and persisting a
/// dollar figure to disk that nothing will ever display is blast radius bought for
/// free.
fn store_shape(body: &serde_json::Value, now: i64) -> serde_json::Value {
    serde_json::json!({
        "cachedUsageUtilization": {
            "fetchedAtMs": now,
            "utilization": { "limits": body.get("limits").cloned().unwrap_or(serde_json::Value::Null) }
        }
    })
}

/// Our own copy, if we have one.
pub fn read_cached() -> Option<crate::limits::Snapshot> {
    let raw = std::fs::read_to_string(cache_path()).ok()?;
    crate::limits::parse(&raw, now_ms())
}

/// Start a refresh in the background if ours has gone off, and return immediately.
///
/// Spawns `aimeter fetch` rather than doing the work here: the caller is usually
/// `aimeter line`, which has a single-digit-millisecond budget and cannot wait for
/// a TLS handshake. Its own process group, so the terminal's Ctrl-C does not kill
/// a fetch mid-write.
pub fn refresh_in_background() {
    if disabled() {
        return;
    }
    let due = read_cached()
        .and_then(|s| s.age_ms)
        .map(|age| Duration::from_millis(age.max(0) as u64) >= refresh_after())
        .unwrap_or(true);
    if !due {
        return;
    }
    if age_of(&attempt_path()).is_some_and(|age| age < refresh_after()) {
        return;
    }

    let Ok(exe) = std::env::current_exe() else { return };
    if let Some(parent) = attempt_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(attempt_path(), b"");

    #[cfg(unix)]
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new(exe);
    command
        .arg("fetch")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    let _ = command.spawn();
}

/* ------------------------------------------------------------------ tests ---- */

#[cfg(test)]
mod tests {
    use super::*;

    /// Exactly what the endpoint returns, including the parts we refuse to keep.
    fn response() -> serde_json::Value {
        serde_json::json!({
            "five_hour": { "utilization": 2.0 },
            "nimbus_quill": null,
            "spend": { "amount_cents": 12_345 },
            "extra_usage": { "enabled": true },
            "limits": [
                { "kind": "session", "percent": 2, "severity": "normal",
                  "resets_at": "2099-01-01T22:39:59.996776+00:00" },
                { "kind": "weekly_all", "percent": 0, "severity": "normal",
                  "resets_at": "2099-01-16T11:59:59.996806+00:00" },
                { "kind": "weekly_scoped", "percent": 0, "severity": "normal",
                  "resets_at": null, "scope": { "model": { "display_name": "Fable" } } }
            ]
        })
    }

    #[test]
    fn the_stored_shape_is_the_one_limits_already_parses() {
        let stored = store_shape(&response(), 1000).to_string();
        let snap = crate::limits::parse(&stored, 1000).expect("our cache parses");
        let labels: Vec<&str> = snap.limits.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["S", "W", "@F"]);
        assert_eq!(snap.limits[0].percent, 2.0);
        assert!(!snap.is_stale());
    }

    /// Nothing reads `spend`, so nothing writes it. A dollar figure landing in a
    /// file on disk should be a decision, not a side effect of wrapping a response.
    #[test]
    fn only_the_limits_are_persisted() {
        let stored = store_shape(&response(), 1000).to_string();
        assert!(!stored.contains("spend"), "{stored}");
        assert!(!stored.contains("amount_cents"), "{stored}");
        assert!(!stored.contains("extra_usage"), "{stored}");
        assert!(!stored.contains("nimbus_quill"), "{stored}");
        assert!(stored.contains("weekly_scoped"), "the part we do read survives");
    }

    /// A response with no `limits` at all stores something that parses to nothing,
    /// which is what makes `limits::read` fall back to Claude Code's cache.
    #[test]
    fn a_response_without_limits_stores_nothing_readable() {
        let stored = store_shape(&serde_json::json!({ "spend": 1 }), 1000).to_string();
        assert!(crate::limits::parse(&stored, 1000).is_none());
    }

    #[test]
    fn refresh_interval_is_overridable() {
        assert_eq!(parse_refresh(None), Duration::from_secs(60));
        assert_eq!(parse_refresh(Some("300")), Duration::from_secs(300));
        // Junk falls back rather than panicking a statusline child.
        assert_eq!(parse_refresh(Some("soon")), Duration::from_secs(60));
        assert_eq!(parse_refresh(Some("")), Duration::from_secs(60));
    }

    /// The off switch has to be hard to set by accident and hard to unset by
    /// accident: an empty or `0` value is how shells spell "not set".
    #[test]
    fn the_off_switch_takes_the_usual_spellings() {
        assert!(!off_switch(None));
        assert!(!off_switch(Some("")));
        assert!(!off_switch(Some("0")));
        assert!(!off_switch(Some("  ")));
        assert!(off_switch(Some("1")));
        assert!(off_switch(Some("true")));
        assert!(off_switch(Some("yes")));
    }
}
