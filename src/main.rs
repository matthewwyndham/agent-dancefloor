//! dancefloor — a terminal dashboard for live Claude Code sessions.

use std::time::Duration;

use dancefloor::{app, discovery, model, ui};

use anyhow::{bail, Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use app::{App, Tab};

const POLL: Duration = Duration::from_millis(120);
const INTERVAL_SECS_DEFAULT: u64 = 2;
const INTERVAL_SECS_MAX: u64 = 3600;

struct Options {
    interval: Duration,
    context_limit: Option<u64>,
    once: bool,
}

fn main() -> Result<()> {
    let options = match parse_args()? {
        Some(options) => options,
        // --help printed its text; that is a successful run, not a refusal.
        None => return Ok(()),
    };

    let home = discovery::claude_home()?;
    if !home.is_dir() {
        bail!("no Claude Code directory at {}", home.display());
    }

    let mut app = App::new(home, options.interval, options.context_limit);
    app.refresh();

    if options.once {
        print_snapshot(&app);
        return Ok(());
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, app)).context("draw")?;

        // Poll on a short tick regardless of the refresh interval, so keys stay
        // responsive while data is re-read at its own pace.
        if event::poll(POLL).context("poll input")? {
            if let Event::Key(key) = event::read().context("read input")? {
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key.code, key.modifiers);
                }
            }
        }

        if app.last_refresh.elapsed() >= app.interval {
            app.refresh();
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    // Help swallows the next keypress so that closing it cannot also act.
    if app.show_help {
        app.show_help = false;
        if !matches!(code, KeyCode::Char('?') | KeyCode::Esc) {
            return;
        }
        return;
    }

    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_previous(),
        KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => app.next_tab(),
        KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => app.previous_tab(),
        KeyCode::Char('1') => app.tab = Tab::Detail,
        KeyCode::Char('2') => app.tab = Tab::Agents,
        KeyCode::Char('3') => app.tab = Tab::Prompt,
        KeyCode::Char('4') => app.tab = Tab::Usage,
        KeyCode::Char('s') => app.cycle_sort(),
        KeyCode::Char('r') => app.refresh(),
        KeyCode::Char('?') => app.show_help = true,
        _ => {}
    }
}

/// `--once` writes one plain-text table and exits, for a shell prompt or a pipe.
fn print_snapshot(app: &App) {
    if let Some(error) = &app.scan_error {
        eprintln!("scan failed: {error}");
    }
    if app.sessions.is_empty() {
        println!("no live Claude Code sessions");
        return;
    }
    println!(
        "{:<7} {:<20} {:<18} {:>5}  {:<16} {:>8}",
        "STATUS", "NAME", "DIR", "CTX", "MODEL", "UP"
    );
    let now = App::now_ms();
    for session in &app.sessions {
        println!(
            "{:<7} {:<20} {:<18} {:>4.0}%  {:<16} {:>8}",
            session.status.label(),
            truncate(&session.name, 20),
            truncate(&session.dir_label(), 18),
            session.context_ratio(app.context_limit) * 100.0,
            truncate(
                session.detail.model.as_deref().unwrap_or("—"),
                16
            ),
            model::duration_short(session.uptime_secs(now)),
        );
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars().take(width.saturating_sub(1)).collect::<String>() + "…"
}

fn parse_args() -> Result<Option<Options>> {
    let mut interval = Duration::from_secs(INTERVAL_SECS_DEFAULT);
    let mut context_limit = None;
    let mut once = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("dancefloor {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--once" => once = true,
            "-i" | "--interval" => {
                let raw = args.next().context("--interval needs a value in seconds")?;
                let secs: u64 = raw
                    .parse()
                    .with_context(|| format!("--interval: {raw} is not a number of seconds"))?;
                if secs == 0 || secs > INTERVAL_SECS_MAX {
                    bail!("--interval must be between 1 and {INTERVAL_SECS_MAX} seconds");
                }
                interval = Duration::from_secs(secs);
            }
            "--context-limit" => {
                let raw = args.next().context("--context-limit needs a token count")?;
                let limit: u64 = raw
                    .parse()
                    .with_context(|| format!("--context-limit: {raw} is not a token count"))?;
                if limit == 0 {
                    bail!("--context-limit must be greater than zero");
                }
                context_limit = Some(limit);
            }
            other => bail!("unknown argument: {other}\nrun with --help for usage"),
        }
    }

    Ok(Some(Options {
        interval,
        context_limit,
        once,
    }))
}

fn print_help() {
    println!(
        "dancefloor {} — a terminal dashboard for live Claude Code sessions

USAGE:
    dancefloor [OPTIONS]

OPTIONS:
    -i, --interval <SECONDS>   Refresh interval, 1 to {INTERVAL_SECS_MAX} (default {INTERVAL_SECS_DEFAULT})
        --context-limit <N>    Pin the context window instead of inferring it
        --once                 Print one plain-text snapshot and exit
    -h, --help                 Show this help
    -V, --version              Show the version

KEYS:
    j k        move between sessions
    tab        next pane, shift-tab previous
    1 2 3 4    Detail, Agents, Prompt, Usage
    s          cycle sort order
    r          refresh now
    ?          help
    q          quit",
        env!("CARGO_PKG_VERSION")
    );
}
