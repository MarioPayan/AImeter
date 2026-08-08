//! Token history, tallied from the transcripts Claude Code writes as it works.
//!
//! `~/.claude/stats-cache.json` looks like it should be the source and is not: it
//! lags a day and only keeps a month. The transcripts under `~/.claude/projects`
//! are complete and current, so they are the source and the stats cache is the
//! cross-check — see `tests::matches_claude_codes_own_tally`.
//!
//! Four things about the tally were established by reconciling against that cache
//! until two independent parsers agreed to the token, and none of them are guesses:
//!
//!   - **Walk recursively, but not into `subagents/workflows/`.** Only 148 of 739
//!     transcripts sit at the top of a project directory; the other 591 are
//!     `<session>/subagents/…`. A flat scan misses every subagent, which is where the
//!     small fast models do most of their work — it reported 27% of the real Fable
//!     number, and agreed with Claude Code on 18 of 58 day/model totals. Including
//!     *everything* does better, 54 of 58, but over-counts the four days that used
//!     the Workflow tool: those agents are already in the parent session's books.
//!     Walking everything except `subagents/workflows/` agrees on all 58.
//!   - **Do not deduplicate.** Repeated `requestId`/`message.id` pairs are separate
//!     accounting entries, not double writes. Collapsing them lands at ~57%.
//!   - **A request costs all four fields.** input + output + cache read + cache
//!     creation. Input and output alone are ~0.1% of the truth; cache traffic is
//!     almost all of it.
//!   - **Bucket by the record's own `timestamp`,** never the file's mtime — a
//!     session that runs past midnight writes both days into one file.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tokens {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_create: u64,
    #[serde(default)]
    pub requests: u64,
}

impl Tokens {
    /// What Claude Code means by "tokens" for a model on a day.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_create
    }

    fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_create += other.cache_create;
        self.requests += other.requests;
    }
}

/// `date (YYYY-MM-DD) -> model -> tokens`. Sorted, because every reader wants it
/// in date order: the sparkline, the windows table, the "last N days" slice.
pub type Daily = BTreeMap<String, BTreeMap<String, Tokens>>;

#[derive(Debug, Serialize, Deserialize)]
pub struct Rollup {
    pub version: u32,
    /// Absolute transcript path -> bytes already tallied. This is the whole of the
    /// incremental story; mtime and size add nothing a byte count does not say.
    #[serde(default)]
    pub files: HashMap<String, u64>,
    #[serde(default)]
    pub daily: Daily,
}

impl Default for Rollup {
    fn default() -> Self {
        Self { version: VERSION, files: HashMap::new(), daily: Daily::new() }
    }
}

/* ------------------------------------------------------------------ paths ---- */

pub fn projects_dir() -> PathBuf {
    crate::limits::home().join(".claude").join("projects")
}

pub fn cache_path() -> PathBuf {
    crate::limits::home().join(".claude").join("ccmeter").join("rollup.json")
}

/* ----------------------------------------------------------------- ingest ---- */

/// One assistant record, reduced to the fields that cost tokens.
#[derive(Deserialize)]
struct Record {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    model: String,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

/// Fold one JSONL line into `daily`. Anything that is not a billable assistant
/// turn — user messages, hook output, the `<synthetic>` placeholder Claude Code
/// writes for local-only turns — is silently skipped.
fn ingest_line(line: &[u8], daily: &mut Daily) {
    let Ok(rec) = serde_json::from_slice::<Record>(line) else { return };
    if rec.kind != "assistant" || rec.timestamp.len() < 10 {
        return;
    }
    let Some(msg) = rec.message else { return };
    let Some(usage) = msg.usage else { return };
    if msg.model.is_empty() || msg.model == "<synthetic>" {
        return;
    }
    let day = &rec.timestamp[..10];
    daily
        .entry(day.to_string())
        .or_default()
        .entry(msg.model)
        .or_default()
        .add(&Tokens {
            input: usage.input_tokens,
            output: usage.output_tokens,
            cache_read: usage.cache_read_input_tokens,
            cache_create: usage.cache_creation_input_tokens,
            requests: 1,
        });
}

/// Read `path` from `from` and fold whole lines into `daily`.
///
/// Returns the new offset, which is the start of the last *incomplete* line: a
/// transcript being written right now ends mid-record, and tallying that fragment
/// would either drop the request or, once the rest arrives, count it twice.
fn ingest_file(path: &Path, from: u64, daily: &mut Daily) -> std::io::Result<u64> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let complete = match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        // Not one whole line yet — leave the offset where it was.
        None => return Ok(from),
    };
    for line in buf[..complete].split(|&b| b == b'\n') {
        if !line.is_empty() {
            ingest_line(line, daily);
        }
    }
    Ok(from + complete as u64)
}

/// `<session>/subagents/workflows/wf_*/agent-*.jsonl` — Workflow-tool agents,
/// whose tokens Claude Code already counts against the session that spawned them.
/// Descending into these is the difference between agreeing with its tally on all
/// 58 recorded day/model totals and on 54.
fn is_workflow_dir(dir: &Path) -> bool {
    dir.file_name().is_some_and(|n| n == "workflows")
        && dir.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "subagents")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                if !is_workflow_dir(&path) {
                    walk(&path, out)
                }
            }
            Ok(t) if t.is_file() && path.extension().is_some_and(|e| e == "jsonl") => out.push(path),
            _ => {}
        }
    }
}

/// Every transcript under `~/.claude/projects` that counts against your usage:
/// sessions and their subagents, but not Workflow-tool agents.
pub fn transcripts(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

impl Rollup {
    pub fn load() -> Self {
        std::fs::read_to_string(cache_path())
            .ok()
            .and_then(|raw| serde_json::from_str::<Rollup>(&raw).ok())
            .filter(|r| r.version == VERSION)
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = cache_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Write-then-rename: a half-written rollup that fails to parse costs a
        // full rebuild, and the TUI saves on a file-change tick.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, &path)
    }

    /// Tally whatever has been appended since the last pass.
    ///
    /// Returns the number of files that had new bytes. A file that shrank or
    /// vanished means the tally can no longer be trusted — its contribution is
    /// already folded into `daily` and cannot be subtracted back out — so the
    /// whole rollup is rebuilt.
    /// `ponytail: rebuild-on-shrink, ~3s for 760MB and transcripts are append-only,
    /// so per-file contributions are not worth storing to make it incremental.`
    pub fn refresh(&mut self, root: &Path) -> usize {
        let files = transcripts(root);
        let present: std::collections::HashSet<&str> =
            files.iter().filter_map(|p| p.to_str()).collect();

        let rewound = self.files.iter().any(|(path, &offset)| {
            !present.contains(path.as_str())
                || std::fs::metadata(path).map(|m| m.len() < offset).unwrap_or(true)
        });
        if rewound {
            self.files.clear();
            self.daily.clear();
        }

        let mut touched = 0;
        for path in &files {
            let Some(key) = path.to_str() else { continue };
            let Ok(size) = std::fs::metadata(path).map(|m| m.len()) else { continue };
            let from = self.files.get(key).copied().unwrap_or(0);
            if size <= from {
                continue;
            }
            match ingest_file(path, from, &mut self.daily) {
                Ok(next) if next > from => {
                    self.files.insert(key.to_string(), next);
                    touched += 1;
                }
                // A file with only a partial line yet: remember nothing, retry next pass.
                Ok(_) => {}
                Err(_) => {}
            }
        }
        touched
    }

    /// Totals per model over the last `days` calendar days, `today` included.
    pub fn window(&self, today: &str, days: usize) -> BTreeMap<String, Tokens> {
        let mut out: BTreeMap<String, Tokens> = BTreeMap::new();
        let Some(from) = days_back(today, days) else { return out };
        // A date range, not `.rev().take(days)`: taking N *entries* silently reaches
        // past N days whenever a day had no activity, so a quiet week would make
        // "30d" cover forty.
        for models in self.daily.range(from..=today.to_string()).map(|(_, m)| m) {
            for (model, tokens) in models {
                out.entry(model.clone()).or_default().add(tokens);
            }
        }
        out
    }

    pub fn all_time(&self) -> BTreeMap<String, Tokens> {
        let mut out: BTreeMap<String, Tokens> = BTreeMap::new();
        for models in self.daily.values() {
            for (model, tokens) in models {
                out.entry(model.clone()).or_default().add(tokens);
            }
        }
        out
    }

    /// Daily totals across all models, oldest first — the sparkline's input.
    ///
    /// Every calendar day in the range gets a bar, including the ones with no
    /// activity: a sparkline that quietly closes its gaps draws a busy month and
    /// a patchy one identically.
    pub fn recent_totals(&self, today: &str, days: usize) -> Vec<(String, u64)> {
        let Some(from) = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d")
            .ok()
            .map(|d| d - chrono::Duration::days(days as i64 - 1))
        else {
            return Vec::new();
        };
        (0..days)
            .map(|i| {
                let date = (from + chrono::Duration::days(i as i64)).format("%Y-%m-%d").to_string();
                let total = self
                    .daily
                    .get(&date)
                    .map(|models| models.values().map(|t| t.total()).sum())
                    .unwrap_or(0);
                (date, total)
            })
            .collect()
    }
}

impl Rollup {
    /// Calendar days from the earliest tallied day through `today`, inclusive.
    /// `None` when nothing has been tallied yet.
    pub fn span_days(&self, today: &str) -> Option<usize> {
        let first = chrono::NaiveDate::parse_from_str(self.daily.keys().next()?, "%Y-%m-%d").ok()?;
        let last = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").ok()?;
        Some(((last - first).num_days() + 1).max(1) as usize)
    }
}

/// The first day of a `days`-long window ending on `today`, inclusive.
fn days_back(today: &str, days: usize) -> Option<String> {
    let day = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").ok()?;
    Some((day - chrono::Duration::days(days as i64 - 1)).format("%Y-%m-%d").to_string())
}

/* ------------------------------------------------------------------ tests ---- */

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ts: &str, model: &str, input: u64, output: u64, read: u64, create: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"model":"{model}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":{read},"cache_creation_input_tokens":{create}}}}}}}"#
        )
    }

    #[test]
    fn a_request_costs_all_four_token_fields() {
        let mut daily = Daily::new();
        ingest_line(line("2026-08-07T10:00:00Z", "claude-opus-5", 2, 208, 21339, 21417).as_bytes(), &mut daily);
        let t = daily["2026-08-07"]["claude-opus-5"];
        assert_eq!(t.total(), 2 + 208 + 21339 + 21417);
        assert_eq!(t.requests, 1);
    }

    #[test]
    fn repeated_requests_accumulate_rather_than_dedupe() {
        let mut daily = Daily::new();
        let l = line("2026-08-07T10:00:00Z", "claude-opus-5", 1, 1, 1, 1);
        ingest_line(l.as_bytes(), &mut daily);
        ingest_line(l.as_bytes(), &mut daily);
        assert_eq!(daily["2026-08-07"]["claude-opus-5"].requests, 2);
    }

    #[test]
    fn skips_everything_that_is_not_a_billable_assistant_turn() {
        let mut daily = Daily::new();
        for raw in [
            r#"{"type":"user","timestamp":"2026-08-07T10:00:00Z"}"#.to_string(),
            r#"{"type":"assistant","timestamp":"2026-08-07T10:00:00Z","message":{"model":"claude-opus-5"}}"#.to_string(),
            line("2026-08-07T10:00:00Z", "<synthetic>", 0, 0, 0, 0),
            "not json".to_string(),
            String::new(),
        ] {
            ingest_line(raw.as_bytes(), &mut daily);
        }
        assert!(daily.is_empty(), "nothing billable, so nothing tallied: {daily:?}");
    }

    /// A session running past midnight puts two days in one file; the record's own
    /// timestamp decides, not the file's.
    #[test]
    fn buckets_by_record_timestamp_across_a_day_boundary() {
        let mut daily = Daily::new();
        ingest_line(line("2026-08-07T23:59:00Z", "claude-opus-5", 0, 10, 0, 0).as_bytes(), &mut daily);
        ingest_line(line("2026-08-08T00:01:00Z", "claude-opus-5", 0, 20, 0, 0).as_bytes(), &mut daily);
        assert_eq!(daily["2026-08-07"]["claude-opus-5"].total(), 10);
        assert_eq!(daily["2026-08-08"]["claude-opus-5"].total(), 20);
    }

    /// The one that matters: a transcript still being appended to ends mid-record,
    /// and the offset must stop before it so the next pass reads it whole exactly once.
    #[test]
    fn a_half_written_line_is_left_for_the_next_pass() {
        let dir = std::env::temp_dir().join(format!("ccmeter-partial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");

        let whole = line("2026-08-07T10:00:00Z", "claude-opus-5", 0, 100, 0, 0);
        let next = line("2026-08-07T10:00:01Z", "claude-opus-5", 0, 500, 0, 0);
        let (head, tail) = next.split_at(next.len() / 2);

        std::fs::write(&path, format!("{whole}\n{head}")).unwrap();
        let mut daily = Daily::new();
        let offset = ingest_file(&path, 0, &mut daily).unwrap();
        assert_eq!(daily["2026-08-07"]["claude-opus-5"].total(), 100, "fragment not counted");
        assert_eq!(offset as usize, whole.len() + 1, "offset stops at the last newline");

        std::fs::write(&path, format!("{whole}\n{head}{tail}\n")).unwrap();
        let offset = ingest_file(&path, offset, &mut daily).unwrap();
        assert_eq!(daily["2026-08-07"]["claude-opus-5"].total(), 600, "counted once, in full");
        assert_eq!(offset, std::fs::metadata(&path).unwrap().len());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Subagents count; Workflow-tool agents do not. Getting this backwards is a
    /// silent 2x over-count on any day that ran a workflow, which is exactly how
    /// it was found.
    #[test]
    fn walks_subagents_but_not_workflow_agents() {
        let root = std::env::temp_dir().join(format!("ccmeter-walk-{}", std::process::id()));
        let session = root.join("-home-user-proj").join("sess-1");
        std::fs::create_dir_all(session.join("subagents").join("workflows").join("wf_abc")).unwrap();
        std::fs::create_dir_all(root.join("-home-user-proj")).unwrap();

        std::fs::write(root.join("-home-user-proj").join("sess-1.jsonl"), "").unwrap();
        std::fs::write(session.join("subagents").join("agent-1.jsonl"), "").unwrap();
        std::fs::write(
            session.join("subagents").join("workflows").join("wf_abc").join("agent-2.jsonl"),
            "",
        )
        .unwrap();
        // A directory that merely shares the name, in the wrong place, still counts.
        std::fs::create_dir_all(root.join("-home-user-workflows")).unwrap();
        std::fs::write(root.join("-home-user-workflows").join("sess-2.jsonl"), "").unwrap();

        let found: Vec<String> = transcripts(&root)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(found.contains(&"sess-1.jsonl".to_string()), "{found:?}");
        assert!(found.contains(&"agent-1.jsonl".to_string()), "subagent kept: {found:?}");
        assert!(found.contains(&"sess-2.jsonl".to_string()), "not a workflow dir: {found:?}");
        assert!(!found.contains(&"agent-2.jsonl".to_string()), "workflow agent skipped: {found:?}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn windows_slice_by_calendar_day() {
        let mut r = Rollup::default();
        for (day, out) in [("2026-08-05", 1u64), ("2026-08-06", 10), ("2026-08-07", 100)] {
            r.daily.entry(day.into()).or_default().insert(
                "claude-opus-5".into(),
                Tokens { output: out, requests: 1, ..Default::default() },
            );
        }
        assert_eq!(r.window("2026-08-07", 1)["claude-opus-5"].total(), 100);
        assert_eq!(r.window("2026-08-07", 2)["claude-opus-5"].total(), 110);
        assert_eq!(r.all_time()["claude-opus-5"].total(), 111);
        // A window anchored mid-history must not see the future.
        assert_eq!(r.window("2026-08-06", 7)["claude-opus-5"].total(), 11);
        assert_eq!(
            r.recent_totals("2026-08-07", 2),
            vec![("2026-08-06".to_string(), 10), ("2026-08-07".to_string(), 100)]
        );
    }

    /// A window is N calendar days, not the N days that happen to have data —
    /// otherwise a quiet stretch silently widens it.
    #[test]
    fn a_gap_in_activity_does_not_widen_the_window() {
        let mut r = Rollup::default();
        for (day, out) in [("2026-06-01", 999u64), ("2026-08-06", 10), ("2026-08-07", 100)] {
            r.daily.entry(day.into()).or_default().insert(
                "claude-opus-5".into(),
                Tokens { output: out, requests: 1, ..Default::default() },
            );
        }
        // Three entries exist, but June is far outside a 7-day window.
        assert_eq!(r.window("2026-08-07", 7)["claude-opus-5"].total(), 110);
        assert_eq!(r.window("2026-08-07", 30)["claude-opus-5"].total(), 110);
        assert_eq!(r.all_time()["claude-opus-5"].total(), 1109);
    }

    /// Days with no activity are real zeroes, and the sparkline must draw them.
    #[test]
    fn quiet_days_are_zero_bars_not_missing_bars() {
        let mut r = Rollup::default();
        r.daily.entry("2026-08-07".into()).or_default().insert(
            "claude-opus-5".into(),
            Tokens { output: 5, requests: 1, ..Default::default() },
        );
        let series = r.recent_totals("2026-08-07", 4);
        assert_eq!(
            series,
            vec![
                ("2026-08-04".into(), 0),
                ("2026-08-05".into(), 0),
                ("2026-08-06".into(), 0),
                ("2026-08-07".into(), 5),
            ]
        );
    }

    /// The real check: our tally against the one Claude Code computed independently
    /// in `~/.claude/stats-cache.json`. Two parsers agreeing to the token is
    /// evidence; a snapshot of our own output would not be.
    ///
    /// Skips itself where there is no real data (CI, a fresh machine) rather than
    /// failing for the wrong reason.
    #[test]
    fn matches_claude_codes_own_tally() {
        let stats = crate::limits::home().join(".claude").join("stats-cache.json");
        let root = projects_dir();
        let (Ok(raw), true) = (std::fs::read_to_string(&stats), root.is_dir()) else {
            eprintln!("skipped: no {} or no transcripts", stats.display());
            return;
        };
        let Ok(stats) = serde_json::from_str::<serde_json::Value>(&raw) else { return };
        // The cache only tallies through this date; today is still being written.
        let Some(last) = stats["lastComputedDate"].as_str() else { return };
        let Some(days) = stats["dailyModelTokens"].as_array() else { return };

        let mut rollup = Rollup::default();
        rollup.refresh(&root);

        let mut checked = 0;
        for day in days {
            let Some(date) = day["date"].as_str() else { continue };
            if date > last {
                continue;
            }
            let Some(expected) = day["tokensByModel"].as_object() else { continue };
            let Some(ours) = rollup.daily.get(date) else { continue };
            for (model, tokens) in expected {
                let want = tokens.as_u64().unwrap_or(0);
                let got = ours.get(model).map(|t| t.total()).unwrap_or(0);
                assert_eq!(got, want, "{date} {model}: tallied {got}, Claude Code says {want}");
                checked += 1;
            }
        }
        assert!(checked > 0, "cross-check found no overlapping days to compare");
        eprintln!("cross-checked {checked} day/model totals against stats-cache.json");
    }
}
