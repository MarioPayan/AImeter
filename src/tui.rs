//! The dashboard: where you stand against your limits, and how you got there.
//!
//! Live without a daemon. CliFlow needs a server because it runs flows; this reads
//! files, so it polls them: re-reading `~/.claude.json` costs about a millisecond
//! and asking 739 transcripts for their size costs a couple more. A `notify`
//! watcher and its thread would buy nothing at that price.
//! `ponytail: poll on a tick, add a watcher only if the file count grows an order of magnitude.`
//!
//! Days are UTC days, and the panel says so. The tally matches Claude Code's own
//! numbers to the token precisely because it buckets on the raw UTC timestamp;
//! re-bucketing into local time would break that agreement to relabel a heading.

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Paragraph, Row, Sparkline, Table};
use ratatui::Frame;
use std::time::{Duration, Instant};

use crate::limits::{Severity, Snapshot};
use crate::rollup::{Rollup, Tokens};

/// How often the two sources are re-read. The limits file changes whenever Claude
/// Code refreshes its cache; the transcripts change while anything is running.
const LIMITS_EVERY: Duration = Duration::from_secs(2);
const ROLLUP_EVERY: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(250);

/* ------------------------------------------------------------------ theme ---- */

/// Semantic roles with a few presets, cycled at runtime with `t`, held in an atomic
/// so `draw` needs no parameter threading. Lifted from CliFlow's TUI.
mod theme {
    use ratatui::style::Color;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CURRENT: AtomicUsize = AtomicUsize::new(0);
    pub const NAMES: [&str; 3] = ["dark", "light", "high-contrast"];

    pub fn cycle() {
        let _ = CURRENT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |i| {
            Some((i + 1) % NAMES.len())
        });
    }
    pub fn name() -> &'static str {
        NAMES[i()]
    }
    fn i() -> usize {
        CURRENT.load(Ordering::Relaxed)
    }

    pub fn accent() -> Color {
        [Color::Cyan, Color::Blue, Color::Rgb(0, 255, 255)][i()]
    }
    pub fn dim() -> Color {
        [Color::DarkGray, Color::Gray, Color::Gray][i()]
    }
    pub fn ok() -> Color {
        [Color::Green, Color::Green, Color::Rgb(0, 255, 0)][i()]
    }
    pub fn warn() -> Color {
        [Color::Yellow, Color::Rgb(176, 120, 0), Color::Rgb(255, 255, 0)][i()]
    }
    pub fn err() -> Color {
        [Color::Red, Color::Red, Color::Rgb(255, 80, 80)][i()]
    }
}

fn severity_colour(s: Severity) -> Color {
    match s {
        Severity::Normal => theme::ok(),
        Severity::Warning => theme::warn(),
        Severity::Critical => theme::err(),
    }
}

/* --------------------------------------------------------------- formatting ---- */

/// Token counts run to eleven digits; nobody reads those. Three significant
/// figures and a magnitude is what a dashboard is for.
pub fn human(n: u64) -> String {
    const K: f64 = 1_000.0;
    let n = n as f64;
    if n < K {
        return format!("{n:.0}");
    }
    for (limit, suffix) in [(K * K, "K"), (K * K * K, "M"), (K * K * K * K, "B")] {
        if n < limit {
            let scaled = n / (limit / K);
            // 704.7M but 70.47M would be noise — keep the width steady instead.
            return if scaled < 10.0 {
                format!("{scaled:.2}{suffix}")
            } else {
                format!("{scaled:.1}{suffix}")
            };
        }
    }
    format!("{:.1}T", n / (K * K * K * K))
}

/// Request counts are read as counts, not magnitudes — `5,247` beats `5.25K`.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "in 18h", "in 42m", "now" — a countdown, not a timestamp, because the only
/// question anyone asks a reset time is how long they have to wait.
fn until(rfc3339: &str) -> String {
    until_from(rfc3339, chrono::Utc::now().timestamp())
}

/// Split out so the countdown can be tested against a fixed clock — asserting on
/// `now()` races the second boundary and fails a few times a day.
fn until_from(rfc3339: &str, now: i64) -> String {
    let Ok(when) = chrono::DateTime::parse_from_rfc3339(rfc3339) else {
        return String::new();
    };
    let secs = (when.timestamp() - now).max(0);
    match secs {
        0 => "now".into(),
        s if s < 3600 => format!("in {}m", s / 60),
        s if s < 86_400 => format!("in {}h{:02}", s / 3600, (s % 3600) / 60),
        s => format!("in {}d", s / 86_400),
    }
}

fn today_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// `claude-opus-5` is the wire name; the column only has room for what varies.
fn short_model(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
}

/* ------------------------------------------------------------------- state ---- */

struct App {
    snapshot: Option<Snapshot>,
    rollup: Rollup,
    today: String,
    limits_read: Instant,
    rollup_read: Instant,
    help: bool,
    notice: Option<String>,
}

impl App {
    fn new() -> Self {
        let mut rollup = Rollup::load();
        rollup.refresh(&crate::rollup::projects_dir());
        let _ = rollup.save();
        Self {
            snapshot: crate::limits::read(),
            rollup,
            today: today_utc(),
            limits_read: Instant::now(),
            rollup_read: Instant::now(),
            help: false,
            notice: None,
        }
    }

    fn refresh_limits(&mut self) {
        self.snapshot = crate::limits::read();
        self.limits_read = Instant::now();
    }

    fn refresh_rollup(&mut self) {
        if self.rollup.refresh(&crate::rollup::projects_dir()) > 0 {
            let _ = self.rollup.save();
        }
        // A session running past midnight must roll the window over with it.
        self.today = today_utc();
        self.rollup_read = Instant::now();
    }
}

/* -------------------------------------------------------------------- draw ---- */

fn block(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::dim()))
        .title(Span::styled(format!(" {title} "), Style::new().fg(theme::accent())))
}

fn draw_limits(frame: &mut Frame, area: Rect, app: &App) {
    let outer = block("Limits");
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let Some(snapshot) = &app.snapshot else {
        frame.render_widget(
            Paragraph::new("no limit data — is Claude Code running?").style(Style::new().fg(theme::dim())),
            inner,
        );
        return;
    };

    let rows = Layout::vertical(
        snapshot.limits.iter().map(|_| Constraint::Length(1)).collect::<Vec<_>>(),
    )
    .split(inner);

    let stale = snapshot.is_stale();
    for (limit, row) in snapshot.limits.iter().zip(rows.iter()) {
        // Percent gets its own column rather than Gauge's centred label: three bars
        // of different lengths put three centred labels at three different offsets,
        // and the whole point of stacking them is to compare them down a column.
        let [label, bar, pct, reset] = Layout::horizontal([
            Constraint::Length(16),
            Constraint::Min(10),
            Constraint::Length(6),
            Constraint::Length(11),
        ])
        .areas(*row);

        frame.render_widget(
            Paragraph::new(limit.label.clone()).style(Style::new().fg(theme::dim())),
            label,
        );

        let colour = if stale { theme::dim() } else { severity_colour(limit.severity) };
        let ratio = (limit.percent / 100.0).clamp(0.0, 1.0);
        // Drawn by hand rather than with `Gauge`: Gauge centres a label over the
        // bar and punches a blank cell there even when the label is empty, which
        // reads as a gap in the fill. Two repeated characters have no such opinion.
        let width = bar.width as usize;
        let filled = ((ratio * width as f64).round() as usize).min(width);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("█".repeat(filled), Style::new().fg(colour)),
                Span::styled("░".repeat(width - filled), Style::new().fg(theme::dim())),
            ])),
            bar,
        );
        frame.render_widget(
            Paragraph::new(format!("{:>4}%", limit.percent.round() as i64))
                .style(Style::new().fg(colour).add_modifier(Modifier::BOLD)),
            pct,
        );

        let when = limit.resets_at.as_deref().map(until).unwrap_or_default();
        frame.render_widget(
            Paragraph::new(format!(" {when}")).style(Style::new().fg(theme::dim())),
            reset,
        );
    }
}

fn draw_today(frame: &mut Frame, area: Rect, app: &App) {
    let outer = block("Today · UTC");
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let today = app.rollup.daily.get(&app.today).cloned().unwrap_or_default();
    let mut models: Vec<(&String, &Tokens)> = today.iter().collect();
    models.sort_by_key(|(_, t)| std::cmp::Reverse(t.total()));

    let mut lines: Vec<Line> = models
        .iter()
        .map(|(model, tokens)| {
            Line::from(vec![
                Span::styled(format!("{:<12}", short_model(model)), Style::new().fg(theme::dim())),
                Span::raw(format!("{:>8}", human(tokens.total()))),
            ])
        })
        .collect();

    if lines.is_empty() {
        lines.push(Line::styled("nothing yet today", Style::new().fg(theme::dim())));
    } else {
        let total: u64 = today.values().map(|t| t.total()).sum();
        let requests: u64 = today.values().map(|t| t.requests).sum();
        lines.push(Line::styled("─".repeat(20), Style::new().fg(theme::dim())));
        lines.push(Line::from(vec![
            Span::styled(format!("{:<12}", "total"), Style::new().fg(theme::accent())),
            Span::styled(format!("{:>8}", human(total)), Style::new().add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{:<12}", "requests"), Style::new().fg(theme::dim())),
            Span::raw(format!("{:>8}", thousands(requests))),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_history(frame: &mut Frame, area: Rect, app: &App) {
    // As many days as fit, but never more history than exists — padding the chart
    // with empty days from before the first transcript makes a busy month look sparse.
    let fits = (area.width.saturating_sub(2) as usize).clamp(7, 60);
    let days = app.rollup.span_days(&app.today).unwrap_or(fits).clamp(7, fits);
    let series = app.rollup.recent_totals(&app.today, days);
    let title = match (series.first(), series.len()) {
        (Some((from, _)), n) => format!("Tokens / day — {n}d from {from}"),
        _ => "Tokens / day".into(),
    };
    let outer = block(&title);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let values: Vec<u64> = series.iter().map(|(_, v)| *v).collect();
    // Every day now yields a bar, so "no data" is a series of zeroes rather than an
    // empty one — a flat row of nothing would otherwise read as thirty quiet days.
    if values.iter().all(|&v| v == 0) {
        frame.render_widget(
            Paragraph::new("no history yet — run `ccmeter backfill`").style(Style::new().fg(theme::dim())),
            inner,
        );
        return;
    }
    let [chart, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Sparkline::default().data(&values).style(Style::new().fg(theme::accent())),
        chart,
    );
    let peak = values.iter().copied().max().unwrap_or(0);
    frame.render_widget(
        Paragraph::new(format!("peak {}/day", human(peak))).style(Style::new().fg(theme::dim())),
        footer,
    );
}

fn draw_windows(frame: &mut Frame, area: Rect, app: &App) {
    let outer = block("Windows");
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let today = app.rollup.window(&app.today, 1);
    let week = app.rollup.window(&app.today, 7);
    let month = app.rollup.window(&app.today, 30);
    let all = app.rollup.all_time();

    // Ordered by lifetime spend so the model you actually use is the first row.
    let mut models: Vec<&String> = all.keys().collect();
    models.sort_by_key(|m| std::cmp::Reverse(all.get(*m).map(|t| t.total()).unwrap_or(0)));

    let cell = |v: u64| {
        Cell::from(format!("{:>10}", if v == 0 { "·".into() } else { human(v) }))
    };
    let rows: Vec<Row> = models
        .iter()
        .map(|model| {
            let get = |w: &std::collections::BTreeMap<String, Tokens>| {
                w.get(*model).map(|t| t.total()).unwrap_or(0)
            };
            Row::new(vec![
                Cell::from(short_model(model).to_string()),
                cell(get(&today)),
                cell(get(&week)),
                cell(get(&month)),
                cell(get(&all)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(12),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(11),
    ];
    frame.render_widget(
        Table::new(rows, widths).header(
            Row::new(vec!["model", "     today", "        7d", "       30d", "       all"])
                .style(Style::new().fg(theme::dim())),
        ),
        inner,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::styled("ccmeter", Style::new().fg(theme::accent()).add_modifier(Modifier::BOLD)),
        Line::raw(""),
        Line::raw("  q / esc   quit"),
        Line::raw("  r         re-read both sources now"),
        Line::raw("  t         cycle theme"),
        Line::raw("  ?         this help"),
        Line::raw(""),
        Line::styled(
            "  limits: ~/.claude.json (Claude Code's cache)",
            Style::new().fg(theme::dim()),
        ),
        Line::styled(
            "  tokens: ~/.claude/projects/**/*.jsonl",
            Style::new().fg(theme::dim()),
        ),
    ];
    let w = 52.min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let box_area = Rect::new((area.width - w) / 2, (area.height - h) / 2, w, h);
    frame.render_widget(ratatui::widgets::Clear, box_area);
    frame.render_widget(Paragraph::new(lines).block(block("help")), box_area);
}

fn draw(frame: &mut Frame, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    // Header: the name, then how current the limits are. Claude Code refreshes its
    // cache on its own slow cadence, so the honest thing is to show the age rather
    // than claim "live" — the number is real, it is just as old as it says.
    let stale = app.snapshot.as_ref().map(|s| s.is_stale()).unwrap_or(true);
    let note = match app.snapshot.as_ref().and_then(|s| s.age_ms) {
        Some(age) => format!("limits {}", crate::limits::ago(age)),
        None => "no limit data".into(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ccmeter ", Style::new().fg(theme::accent()).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{} {note}", if stale { "○" } else { "●" }),
                Style::new().fg(if stale { theme::warn() } else { theme::dim() }),
            ),
        ])),
        header,
    );

    let limit_rows = app.snapshot.as_ref().map(|s| s.limits.len() as u16).unwrap_or(1);
    let [top, middle, bottom] = Layout::vertical([
        Constraint::Length(limit_rows + 2),
        Constraint::Min(6),
        Constraint::Length(9),
    ])
    .areas(body);

    draw_limits(frame, top, app);

    let [today, history] =
        Layout::horizontal([Constraint::Length(26), Constraint::Min(20)]).areas(middle);
    draw_today(frame, today, app);
    draw_history(frame, history, app);
    draw_windows(frame, bottom, app);

    let hint = app.notice.clone().unwrap_or_else(|| "q quit · r refresh · t theme · ? help".into());
    frame.render_widget(
        Paragraph::new(format!(" {hint}   [{}]", theme::name())).style(Style::new().fg(theme::dim())),
        footer,
    );

    if app.help {
        draw_help(frame, frame.area());
    }
}

/* -------------------------------------------------------------------- loop ---- */

pub fn main() -> std::io::Result<()> {
    // A panic inside draw otherwise leaves the shell in raw mode — no echo, garbled.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    let mut app = App::new();
    let mut terminal = ratatui::init();

    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    loop {
        if app.limits_read.elapsed() >= LIMITS_EVERY {
            app.refresh_limits();
        }
        if app.rollup_read.elapsed() >= ROLLUP_EVERY {
            app.refresh_rollup();
        }

        terminal.draw(|frame| draw(frame, app))?;

        if !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        // Help is modal: q still quits, anything else dismisses it.
        if app.help {
            app.help = matches!(key.code, KeyCode::Char('?'));
            if !matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                continue;
            }
        }
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('t') => theme::cycle(),
            KeyCode::Char('?') => app.help = true,
            KeyCode::Char('r') => {
                app.refresh_limits();
                app.refresh_rollup();
                app.notice = Some("re-read".into());
            }
            _ => app.notice = None,
        }
    }
}

/* ------------------------------------------------------------------ tests ---- */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_keeps_three_significant_figures() {
        assert_eq!(human(0), "0");
        assert_eq!(human(999), "999");
        assert_eq!(human(1_000), "1.00K");
        assert_eq!(human(48_113_285), "48.1M");
        assert_eq!(human(704_738_522), "704.7M");
        assert_eq!(human(3_210_000_000), "3.21B");
        assert_eq!(human(13_900_000_000), "13.9B");
    }

    #[test]
    fn until_counts_down_and_never_goes_negative() {
        // A real value from ~/.claude.json, including its sub-second precision.
        let reset = "2026-08-09T11:59:59.728746+00:00";
        let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp();

        assert_eq!(until_from(reset, at("2026-08-09T11:59:59+00:00")), "now");
        assert_eq!(until_from(reset, at("2026-08-09T11:17:59+00:00")), "in 42m");
        assert_eq!(until_from(reset, at("2026-08-08T17:59:59+00:00")), "in 18h00");
        assert_eq!(until_from(reset, at("2026-08-07T11:00:00+00:00")), "in 2d");
        // Already past: a countdown never runs backwards.
        assert_eq!(until_from(reset, at("2026-08-10T00:00:00+00:00")), "now");
        assert_eq!(until_from("not a timestamp", 0), "");
    }

    #[test]
    fn model_names_lose_the_vendor_prefix() {
        assert_eq!(short_model("claude-opus-5"), "opus-5");
        assert_eq!(short_model("gpt-4"), "gpt-4");
    }

    /// The whole dashboard rendered into a fixed buffer — catches panics from
    /// layout arithmetic (a zero-width chunk, an area smaller than its content)
    /// that only ever show up at an awkward terminal size.
    #[test]
    fn draws_at_awkward_sizes_without_panicking() {
        let snapshot = crate::limits::parse(
            r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
                {"kind":"session","percent":4,"severity":"normal","resets_at":"2030-01-01T00:00:00Z"},
                {"kind":"weekly_all","percent":77,"severity":"warning"}
            ]}}}"#,
            1,
        );
        let mut rollup = Rollup::default();
        rollup.daily.entry("2026-08-07".into()).or_default().insert(
            "claude-opus-5".into(),
            Tokens { output: 500, requests: 3, ..Default::default() },
        );
        let app = App {
            snapshot,
            rollup,
            today: "2026-08-07".into(),
            limits_read: Instant::now(),
            rollup_read: Instant::now(),
            help: false,
            notice: None,
        };

        for (w, h) in [(120, 40), (80, 24), (40, 12), (20, 8)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
        }
    }

    /// Render one frame and hand back its rows as plain text.
    fn frame_rows(app: &App, w: u16, h: u16) -> Vec<String> {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    /// The column a string starts at, counting terminal cells rather than bytes.
    /// `│` and `█` are three bytes each, so `str::find` answers a different
    /// question than "which column is this in" — and got these tests wrong once.
    fn col(row: &str, needle: &str) -> usize {
        let byte = row.find(needle).unwrap_or_else(|| panic!("no {needle:?} in {row:?}"));
        row[..byte].chars().count()
    }

    /// Where a string *ends*, which is what right-aligned columns share.
    fn end_col(row: &str, needle: &str) -> usize {
        col(row, needle) + needle.chars().count()
    }

    fn row_starting_with<'a>(rows: &'a [String], label: &str) -> &'a str {
        rows.iter()
            .map(|r| r.as_str())
            .find(|r| r.trim_start_matches(['│', ' ']).starts_with(label))
            .unwrap_or_else(|| panic!("no row starting with {label:?}"))
    }

    fn sample_app() -> App {
        let snapshot = crate::limits::parse(
            r#"{"cachedUsageUtilization":{"fetchedAtMs":1,"utilization":{"limits":[
                {"kind":"session","percent":4,"severity":"normal","resets_at":"2030-01-01T00:00:00Z"},
                {"kind":"weekly_all","percent":77,"severity":"warning","resets_at":"2030-01-01T00:00:00Z"},
                {"kind":"weekly_scoped","percent":100,"severity":"critical",
                 "scope":{"model":{"display_name":"Fable"}}}
            ]}}}"#,
            1,
        );
        // One day in each window, so today / 7d / 30d / all are four distinct
        // numbers and a column that drifts cannot accidentally match its neighbour.
        let mut rollup = Rollup::default();
        for (day, opus, fable) in [
            ("2026-05-01", 1_000_000_000u64, 0u64), // all only
            ("2026-07-20", 400_000_000, 0),         // 30d
            ("2026-08-06", 621_995_594, 30_446_597), // 7d
            ("2026-08-07", 929_600_000, 140_300_000), // today
        ] {
            let e = rollup.daily.entry(day.into()).or_default();
            e.insert("claude-opus-5".into(), Tokens { output: opus, requests: 4_000, ..Default::default() });
            if fable > 0 {
                e.insert("claude-fable-5".into(), Tokens { output: fable, requests: 1_247, ..Default::default() });
            }
        }
        App {
            snapshot,
            rollup,
            today: "2026-08-07".into(),
            limits_read: Instant::now(),
            rollup_read: Instant::now(),
            help: false,
            notice: None,
        }
    }

    /// Three stacked bars exist to be compared down a column, so the percentages
    /// must share one. Gauge's own centred label puts each at a different offset,
    /// which is why it is suppressed in favour of a dedicated column.
    #[test]
    fn limit_percentages_share_a_column() {
        let rows = frame_rows(&sample_app(), 118, 34);
        let (session, weekly, scoped) = (
            row_starting_with(&rows, "5h"),
            row_starting_with(&rows, "7d"),
            row_starting_with(&rows, "Fable"),
        );
        // Right-aligned in their own column, so 4%, 77% and 100% all end together.
        let e = end_col(session, "4%");
        assert_eq!(e, end_col(weekly, "77%"), "{session:?}\n{weekly:?}");
        assert_eq!(e, end_col(scoped, "100%"), "{session:?}\n{scoped:?}");
        // The countdown sits to the right of the percent, not centred inside the bar.
        assert!(col(session, "in ") > e, "{session:?}");
    }

    /// The windows table is five numeric columns; if they drift the table is noise.
    #[test]
    fn window_columns_line_up_under_their_headers() {
        let rows = frame_rows(&sample_app(), 118, 34);
        // "Today" lists opus-5 too, so anchor on the header and read below it.
        let head_at = rows.iter().position(|r| r.contains("today") && r.contains("30d")).unwrap();
        let header = rows[head_at].as_str();
        let opus = rows[head_at + 1..]
            .iter()
            .find(|r| r.contains("opus-5"))
            .unwrap_or_else(|| panic!("no opus row under the header"));

        for (head, value) in [("today", "929.6M"), ("7d", "1.55B"), ("30d", "1.95B"), ("all", "2.95B")] {
            assert_eq!(
                end_col(header, head),
                end_col(opus, value),
                "{head} column drifted\n{header:?}\n{opus:?}"
            );
        }
    }

    #[test]
    fn request_counts_are_grouped_not_scaled() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(5_247), "5,247");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert!(frame_rows(&sample_app(), 118, 34).iter().any(|r| r.contains("5,247")));
    }

    #[test]
    fn empty_state_says_so_instead_of_rendering_zeroes() {
        let app = App {
            snapshot: None,
            rollup: Rollup::default(),
            today: "2026-08-07".into(),
            limits_read: Instant::now(),
            rollup_read: Instant::now(),
            help: false,
            notice: None,
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(text.contains("is Claude Code running?"), "names the cause: {text}");
        assert!(text.contains("ccmeter backfill"), "names the fix");
    }
}
