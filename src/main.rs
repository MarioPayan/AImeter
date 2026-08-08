//! ccmeter — Claude Code usage, from the files Claude Code already writes.
//!
//! Three entry points over two data sources: `~/.claude.json` for where you stand
//! against your limits right now, and the transcripts under `~/.claude/projects`
//! for how you got there.

mod limits;
mod line;
mod rollup;
mod tui;

const HELP: &str = "\
ccmeter — Claude Code usage

USAGE:
    ccmeter [tui]     the dashboard (default)
    ccmeter line      one statusline segment, then exit
    ccmeter backfill  tally every transcript and cache the result

The dashboard and the segment both refresh the tally themselves; `backfill` only
exists so the first run's few seconds happen when you asked for them rather than
the first time you open the TUI.
";

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("line") => line::main(),
        Some("backfill") => backfill(),
        Some("-h") | Some("--help") | Some("help") => print!("{HELP}"),
        Some("tui") | None => {
            if let Err(e) = tui::main() {
                eprintln!("ccmeter: {e}");
                std::process::exit(1);
            }
        }
        Some(other) => {
            eprintln!("ccmeter: unknown command \"{other}\"\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

fn backfill() {
    let root = rollup::projects_dir();
    if !root.is_dir() {
        eprintln!("ccmeter: no transcripts at {}", root.display());
        std::process::exit(1);
    }
    let started = std::time::Instant::now();
    let mut r = rollup::Rollup::load();
    let touched = r.refresh(&root);
    if let Err(e) = r.save() {
        eprintln!("ccmeter: could not write {}: {e}", rollup::cache_path().display());
        std::process::exit(1);
    }
    let days = r.daily.len();
    let total: u64 = r.all_time().values().map(|t| t.total()).sum();
    println!(
        "read {touched} transcript{} in {:.1}s — {days} days, {} tokens",
        if touched == 1 { "" } else { "s" },
        started.elapsed().as_secs_f64(),
        tui::human(total),
    );
}
