//! AImeter — Claude Code usage as a statusline segment.
//!
//! One job: print where you stand against your limits, fast enough to run on every
//! statusline render. Three sources feed it — the payload Claude Code pipes in, the
//! usage endpoint, and Claude Code's own `~/.claude.json` cache — and `line.rs`
//! ranks them.

mod fetch;
mod limits;
mod line;

const HELP: &str = "\
AImeter — Claude Code usage, as a statusline segment

USAGE:
    aimeter line [--bar]   one statusline segment, then exit
    aimeter fetch          ask the usage endpoint for current limits, now
    aimeter --version      print the version and exit

`--bar` adds a fill block beside each percentage. Off by default: the digits
already say what the block would, and only the digits are precise.

A trailing `↑` means a newer release exists. That check runs once a day, needs
no credentials, and re-running the install command is what applies it.

`line` refreshes the limits itself, in a background child at most once a minute,
and prints whatever is already on disk rather than waiting for the network.
`fetch` exists so you can force that refresh, or see why it is failing.

`fetch` reads the OAuth token in ~/.claude/.credentials.json to call an endpoint
Anthropic does not document. It never writes that file, and every failure falls
back to Claude Code's own cache. Set AIMETER_NO_FETCH to stop it reading the
token at all; everything still works, only the model-scoped limit ages. See
README.md.

ENVIRONMENT:
    AIMETER_ASCII            1 forces plain-ASCII glyphs, 0 forbids the fallback;
                             unset, a non-UTF-8 locale switches it on by itself
    AIMETER_NO_FETCH         no network at all: no token read, no update check
    AIMETER_NO_UPDATE_CHECK  never ask GitHub whether there is a newer release
    AIMETER_REFRESH_SECS     how stale the limits may get before a refresh (60)
    NO_COLOR                 print the segment without escape codes
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // The only flag, so `contains` beats a parser. If a second one ever appears,
        // that is the moment to reach for one — not before.
        Some("line") => line::main(args.iter().any(|a| a == "--bar")),
        Some("-V") | Some("--version") | Some("version") => {
            println!("AImeter {}", env!("CARGO_PKG_VERSION"))
        }
        // This process is the detached child the statusline spawns, so it is the
        // one place a network call costs nobody anything. The update check rides
        // along and rate-limits itself to once a day.
        Some("fetch") => {
            fetch::check_for_update();
            match fetch::fetch_now() {
                // Quiet on success when nobody is watching: this normally runs as
                // a detached child with its output pointed at /dev/null.
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
            }
        }
        // Bare `aimeter` prints help rather than guessing. `line` is the only thing
        // anyone runs unattended, and it is never run without being asked for.
        Some("-h") | Some("--help") | Some("help") | None => print!("{HELP}"),
        Some(other) => {
            eprintln!("aimeter: unknown command \"{other}\"\n\n{HELP}");
            std::process::exit(2);
        }
    }
}
