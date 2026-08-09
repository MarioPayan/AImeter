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
//! This uses an endpoint issued to another application. It works today and may
//! not tomorrow — every failure here is silent and falls back to the cache.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// How old our copy may be before a read triggers a background refresh.
pub fn refresh_after() -> Duration {
    std::env::var("AIMETER_REFRESH_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(60))
}

pub fn cache_path() -> PathBuf {
    crate::rollup::data_dir().join("usage.json")
}

/// Touched before every attempt, successful or not.
///
/// The statusline runs every ten seconds; without this, an unreachable network
/// would spawn six doomed children a minute forever. Rate-limiting *attempts*
/// rather than successes is what makes the offline case quiet.
fn attempt_path() -> PathBuf {
    crate::rollup::data_dir().join("usage.attempt")
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

/// Fetch once and write the result. Returns the raw utilization JSON on success.
pub fn fetch_now() -> Result<String, String> {
    let token = access_token().ok_or("no usable Claude Code token")?;
    let body: serde_json::Value = ureq::get(URL)
        .set("authorization", &format!("Bearer {token}"))
        .set("accept", "application/json")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| match e {
            // Not an error worth shouting about: Claude Code will refresh its own
            // token when it next runs, and we pick the new one up for free.
            ureq::Error::Status(401, _) => "token rejected (401) — Claude Code will refresh it".into(),
            ureq::Error::Status(code, _) => format!("usage endpoint returned {code}"),
            ureq::Error::Transport(t) => format!("cannot reach the usage endpoint: {t}"),
        })?
        .into_json()
        .map_err(|e| format!("usage endpoint sent something that is not JSON: {e}"))?;

    // Stored in exactly the shape Claude Code uses, so `limits::parse` serves both
    // files and neither source needs its own reader.
    let wrapped = serde_json::json!({
        "cachedUsageUtilization": { "fetchedAtMs": now_ms(), "utilization": body }
    });
    let text = serde_json::to_string(&wrapped).map_err(|e| e.to_string())?;

    let path = cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(text)
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

    #[test]
    fn the_stored_shape_is_the_one_limits_already_parses() {
        // Exactly what the endpoint returns, wrapped the way `fetch_now` wraps it.
        let body = serde_json::json!({
            "five_hour": { "utilization": 2.0 },
            "nimbus_quill": null,
            "limits": [
                { "kind": "session", "percent": 2, "severity": "normal",
                  "resets_at": "2099-01-01T22:39:59.996776+00:00" },
                { "kind": "weekly_all", "percent": 0, "severity": "normal",
                  "resets_at": "2099-01-16T11:59:59.996806+00:00" },
                { "kind": "weekly_scoped", "percent": 0, "severity": "normal",
                  "resets_at": null, "scope": { "model": { "display_name": "Fable" } } }
            ]
        });
        let stored = serde_json::json!({
            "cachedUsageUtilization": { "fetchedAtMs": 1000, "utilization": body }
        })
        .to_string();

        let snap = crate::limits::parse(&stored, 1000).expect("our cache parses");
        let labels: Vec<&str> = snap.limits.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["5h", "7d", "Fable"]);
        assert_eq!(snap.limits[0].percent, 2.0);
        assert!(!snap.is_stale());
    }

    #[test]
    fn refresh_interval_is_overridable() {
        assert_eq!(refresh_after(), Duration::from_secs(60));
        std::env::set_var("AIMETER_REFRESH_SECS", "300");
        assert_eq!(refresh_after(), Duration::from_secs(300));
        std::env::remove_var("AIMETER_REFRESH_SECS");
    }
}
