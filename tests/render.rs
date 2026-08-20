//! Drives the real render path over a test backend.
//!
//! ratatui's layout helpers panic when a fixed-size region cannot fit, so a new
//! panel with an unguarded `Constraint::Length` breaks the app on a small
//! terminal and nowhere else. These sizes are the ones that would catch it.

use std::path::PathBuf;
use std::time::Duration;

use dancefloor::app::{App, Tab};
use dancefloor::model::{
    ContextUsage, Detail, ProcStat, PullRequest, Session, Status, Subagent, TailTotals, Worktree,
};
use dancefloor::ui;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const SIZES: [(u16, u16); 6] = [(20, 8), (40, 12), (80, 24), (120, 40), (200, 60), (1, 1)];

fn populated_session() -> Session {
    Session {
        pid: 4242,
        session_id: "00000000-0000-4000-8000-000000000000".into(),
        cwd: PathBuf::from("/Users/someone/code/dancefloor"),
        name: "dancefloor-be".into(),
        status: Status::Busy,
        version: "2.1.237".into(),
        kind: "interactive".into(),
        entrypoint: "cli".into(),
        started_at_ms: App::now_ms() - 90_000,
        status_updated_at_ms: App::now_ms() - 5_000,
        proc: Some(ProcStat {
            rss_kib: 431_792,
            cpu_percent: 7.3,
        }),
        detail: Detail {
            transcript: Some(PathBuf::from("/tmp/transcript.jsonl")),
            transcript_age_secs: Some(12),
            model: Some("claude-opus-5".into()),
            effort: Some("high".into()),
            service_tier: Some("standard".into()),
            usage: Some(ContextUsage {
                input: 2,
                cache_read: 104_287,
                cache_creation: 743,
                output: 811,
            }),
            usage_peak: 120_000,
            totals: TailTotals {
                assistant_messages: 86,
                user_messages: 50,
                output_tokens: 40_000,
                thinking_tokens: 1_200,
                cache_creation_tokens: 9_000,
                web_searches: 2,
            },
            title: Some("Add the subagents pane".into()),
            git_branch: Some("main".into()),
            permission_mode: Some("auto".into()),
            mode: Some("normal".into()),
            worktree: Some(Worktree {
                name: "fix/thing".into(),
                branch: "worktree-fix+thing".into(),
                original_branch: "main".into(),
                path: "/Users/someone/code/web-shop/.claude/worktrees/fix+thing".into(),
            }),
            pull_request: Some(PullRequest {
                number: 863,
                url: "https://github.com/example/repo/pull/863".into(),
                repository: "example/repo".into(),
            }),
            last_prompt: Some("add a pane for subagents".into()),
            subagents: vec![Subagent {
                name: "code-review".into(),
                agent_type: "general-purpose".into(),
                description: "/code-review".into(),
                spawn_depth: 1,
                age_secs: Some(240),
                bytes: 18_432,
            }],
            read_error: None,
        },
    }
}

fn app_with(sessions: Vec<Session>) -> App {
    let mut app = App::new(PathBuf::from("/nonexistent"), Duration::from_secs(2), None);
    app.sessions = sessions;
    app
}

fn render(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            if let Some(cell) = buffer.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn renders_every_pane_at_every_size() {
    let app_states = [
        app_with(Vec::new()),
        app_with(vec![populated_session()]),
        app_with(vec![populated_session(), populated_session()]),
    ];

    for mut app in app_states {
        for tab in Tab::ALL {
            app.tab = tab;
            for (width, height) in SIZES {
                render(&app, width, height);
            }
        }
    }
}

#[test]
fn renders_help_overlay_at_every_size() {
    let mut app = app_with(vec![populated_session()]);
    app.show_help = true;
    for (width, height) in SIZES {
        render(&app, width, height);
    }
}

#[test]
fn detail_pane_shows_the_session_facts() {
    let app = app_with(vec![populated_session()]);
    let screen = render(&app, 140, 40);

    assert!(screen.contains("dancefloor-be"), "name missing:\n{screen}");
    assert!(screen.contains("claude-opus-5"), "model missing:\n{screen}");
    assert!(screen.contains("#863"), "pr missing:\n{screen}");
    assert!(screen.contains("main"), "branch missing:\n{screen}");
    // 105_843 of an inferred 200k window, and the ~ must say it was inferred.
    assert!(screen.contains("105k / 200k~"), "context missing:\n{screen}");
    assert!(screen.contains("53%"), "context percentage missing:\n{screen}");
}

#[test]
fn empty_state_explains_itself_rather_than_showing_a_blank_pane() {
    let app = app_with(Vec::new());
    let screen = render(&app, 80, 24);
    assert!(
        screen.contains("No live sessions"),
        "empty hint missing:\n{screen}"
    );
}
