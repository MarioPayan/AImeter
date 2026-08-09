//! aimeter — Claude Code usage, from the files Claude Code already writes.
//!
//! Three entry points over two data sources: `~/.claude.json` for where you stand
//! against your limits right now, and the transcripts under `~/.claude/projects`
//! for how you got there.

mod fetch;
mod limits;
mod line;
mod rollup;
mod tui;

const HELP: &str = "\
aimeter — Claude Code usage

USAGE:
    aimeter [tui]     the dashboard (default)
    aimeter line      one statusline segment, then exit
    aimeter fetch     ask the usage endpoint for current limits, now
    aimeter backfill  tally every transcript and cache the result

The dashboard and the segment refresh both halves themselves — limits in a
background child at most once a minute, the token tally inline. `fetch` and
`backfill` exist so you can force either, or see why one is failing.

ENVIRONMENT:
    AIMETER_REFRESH_SECS   how stale the limits may get before a refresh (60)
";

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("line") => line::main(),
        Some("fetch") => match fetch::fetch_now() {
            // Quiet on success when nobody is watching: this normally runs as a
            // detached child with its output pointed at /dev/null.
            Ok(_) => {
                if let Some(s) = limits::read() {
                    for limit in &s.limits {
                        println!("{:<8} {:>5.0}%", limit.label, limit.percent);
                    }
                }
            }
            Err(e) => {
                eprintln!("aimeter: {e}");
                std::process::exit(1);
            }
        },
        Some("backfill") => backfill(),
        Some("-h") | Some("--help") | Some("help") => print!("{HELP}"),
        Some("tui") | None => {
            if let Err(e) = tui::main() {
                eprintln!("aimeter: {e}");
                std::process::exit(1);
            }
        }
        Some(other) => {
            eprintln!("aimeter: unknown command \"{other}\"\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

fn backfill() {
    let root = rollup::projects_dir();
    if !root.is_dir() {
        eprintln!("aimeter: no transcripts at {}", root.display());
        std::process::exit(1);
    }
    let started = std::time::Instant::now();
    let mut r = rollup::Rollup::load();
    let touched = r.refresh(&root);
    if let Err(e) = r.save() {
        eprintln!("aimeter: could not write {}: {e}", rollup::cache_path().display());
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
