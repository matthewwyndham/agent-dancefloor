//! Drives the real render path over a test backend.
//!
//! ratatui's layout helpers panic when a fixed-size region cannot fit, so a new
//! panel with an unguarded `Constraint::Length` breaks the app on a small
//! terminal and nowhere else. These sizes are the ones that would catch it.

use std::path::PathBuf;
use std::time::Duration;

use dancefloor::app::{App, Focus, Tab};
use dancefloor::model::{
    Activity, ContextUsage, Detail, Driver, Limits, ProcStat, PullRequest, Session, Status,
    Subagent, TailTotals, ToolCall, Turn, Worktree,
};
use dancefloor::ui;
use ratatui::backend::TestBackend;
use ratatui::style::Color;
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
        configured_model: None,
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
            long_context_models: Vec::new(),
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
            activity: Activity {
                recap: Some("Added the pane and wired its key. Next: run the tests.".into()),
                driver: Some(Driver::Skill("pstack:poteto-mode".into())),
                last_turn: Some(Turn {
                    duration_ms: 418_374,
                    messages: 135,
                }),
                tools: vec![
                    ToolCall {
                        name: "Edit".into(),
                        summary: String::new(),
                        detail: "/Users/someone/code/dancefloor/src/ui/activity.rs".into(),
                    },
                    ToolCall {
                        name: "Bash".into(),
                        summary: "Run the test suite".into(),
                        detail: "cargo test --quiet 2>&1 | tail -20".into(),
                    },
                ],
                files: vec![PathBuf::from(
                    "/Users/someone/code/dancefloor/src/ui/activity.rs",
                )],
            },
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
    let mut app = App::new(
        PathBuf::from("/nonexistent"),
        Duration::from_secs(2),
        Limits::default(),
    );
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
            // Focus decides what the pane draws, so each state is its own size
            // sweep. The open tool call is a popup sized off the terminal.
            for focus in [Focus::Sessions, Focus::Pane, Focus::Tool] {
                app.focus = focus;
                for (width, height) in SIZES {
                    render(&app, width, height);
                }
            }
        }
    }
}

/// A session with more tool calls than the pane has rows, so scrolling has
/// something to do.
fn busy_session(calls: usize) -> Session {
    let mut session = populated_session();
    session.detail.activity.tools = (0..calls)
        .map(|index| ToolCall {
            name: "Bash".into(),
            summary: format!("step {index}"),
            detail: format!("cargo run -- --step {index}"),
        })
        .collect();
    session
}

#[test]
fn a_tool_row_shows_the_summary_and_the_command() {
    let mut app = app_with(vec![populated_session()]);
    app.tab = Tab::Activity;

    let screen = render(&app, 140, 40);
    assert!(
        screen.contains("Run the test suite"),
        "summary missing:\n{screen}"
    );
    assert!(
        screen.contains("cargo test --quiet"),
        "command missing:\n{screen}"
    );
}

#[test]
fn focusing_the_pane_scrolls_the_list_under_the_cursor() {
    let mut app = app_with(vec![busy_session(200)]);
    app.tab = Tab::Activity;

    let screen = render(&app, 140, 24);
    assert!(screen.contains("tools  200"), "count missing:\n{screen}");
    assert!(screen.contains("enter to browse"), "hint missing:\n{screen}");
    assert!(screen.contains("step 199"), "newest call missing:\n{screen}");

    app.focus = Focus::Pane;
    let screen = render(&app, 140, 24);
    assert!(screen.contains("1 of 200"), "position missing:\n{screen}");
    assert!(screen.contains("Next: run the tests"), "recap gone:\n{screen}");

    for _ in 0..80 {
        app.select_next_tool();
    }
    let screen = render(&app, 140, 24);
    // The readout is pinned, so it survives the scroll that took the recap.
    assert!(screen.contains("81 of 200"), "position missing:\n{screen}");
    assert!(
        !screen.contains("Next: run the tests"),
        "header did not scroll away:\n{screen}"
    );
    assert!(screen.contains("step 119"), "cursor row missing:\n{screen}");
}

/// The cursor has to be visible, not merely tracked. This reads the cell colours
/// back out, because a selection that renders in the default background is the
/// same as no selection at all.
#[test]
fn the_cursor_row_is_drawn_as_a_selected_row() {
    let mut app = app_with(vec![busy_session(40)]);
    app.tab = Tab::Activity;

    assert!(
        selection_bg(&app, "step 39").is_none(),
        "unfocused pane must not draw a cursor"
    );

    app.focus = Focus::Pane;
    assert_eq!(selection_bg(&app, "step 39"), Some(Color::Indexed(238)));
    assert!(
        selection_bg(&app, "step 38").is_none(),
        "only the cursor row is selected"
    );

    app.select_next_tool();
    assert_eq!(selection_bg(&app, "step 38"), Some(Color::Indexed(238)));
}

/// The background of the row holding `needle`, or None when it is the default.
fn selection_bg(app: &App, needle: &str) -> Option<Color> {
    let (width, height) = (140u16, 30u16);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();

    for y in 0..height {
        let mut row = String::new();
        for x in 0..width {
            row.push_str(buffer.cell((x, y)).expect("cell").symbol());
        }
        if !row.contains(needle) {
            continue;
        }
            let bg = buffer.cell((row.find(needle).unwrap() as u16, y)).unwrap().bg;
        return (bg != Color::Reset).then_some(bg);
    }
    panic!("no row contains {needle}");
}

#[test]
fn the_cursor_stops_at_both_ends_of_the_list() {
    let mut app = app_with(vec![busy_session(3)]);
    app.focus = Focus::Pane;

    app.select_previous_tool();
    assert_eq!(app.tool_cursor, 0);
    for _ in 0..10 {
        app.select_next_tool();
    }
    assert_eq!(app.tool_cursor, 2);
}

#[test]
fn an_open_tool_call_shows_the_command_in_full() {
    let mut session = populated_session();
    let long = format!("cargo test {}", "--verbose ".repeat(30));
    session.detail.activity.tools = vec![ToolCall {
        name: "Bash".into(),
        summary: "Run the suite loudly".into(),
        detail: long.clone(),
    }];

    let mut app = app_with(vec![session]);
    app.tab = Tab::Activity;
    app.focus = Focus::Pane;

    // The row has space for one line of it, so the row is cut.
    let screen = render(&app, 90, 30);
    assert!(!screen.contains(&long), "row was not cut:\n{screen}");

    app.open_tool();
    let screen = render(&app, 90, 30);
    assert!(
        screen.contains("Run the suite loudly"),
        "summary missing:\n{screen}"
    );
    assert!(screen.contains("y to copy"), "copy hint missing:\n{screen}");
    // Wrapped across the popup, so the tail is what proves it is all there.
    assert!(
        screen.contains("--verbose --verbose"),
        "command not shown in full:\n{screen}"
    );
}

/// Esc walks back out one level at a time rather than quitting from inside.
#[test]
fn focus_moves_in_and_back_out() {
    let mut app = app_with(vec![busy_session(3)]);
    assert_eq!(app.focus, Focus::Sessions);

    app.focus_pane();
    assert_eq!(app.focus, Focus::Pane);
    app.open_tool();
    assert_eq!(app.focus, Focus::Tool);
    app.close_tool();
    assert_eq!(app.focus, Focus::Pane);
    app.focus_sessions();
    assert_eq!(app.focus, Focus::Sessions);
}

/// Moving to another session must not carry the old session's cursor across.
#[test]
fn changing_session_resets_the_tool_cursor() {
    let mut app = app_with(vec![busy_session(50), busy_session(50)]);
    app.focus = Focus::Pane;
    for _ in 0..20 {
        app.select_next_tool();
    }
    assert_eq!(app.tool_cursor, 20);

    app.select_next();
    assert_eq!(app.tool_cursor, 0);
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
    assert!(
        screen.contains("105k / 200k~"),
        "context missing:\n{screen}"
    );
    assert!(
        screen.contains("53%"),
        "context percentage missing:\n{screen}"
    );
}

/// The bug this guards: a 1M session under 200k of usage was measured against
/// the standard window, so 169k read as 85% in alarm red instead of 17%.
#[test]
fn a_long_window_is_believed_before_usage_proves_it() {
    let mut session = populated_session();
    session.detail.usage = Some(ContextUsage {
        input: 2,
        cache_read: 163_349,
        cache_creation: 5_805,
        output: 300,
    });
    session.detail.usage_peak = 169_456;

    // Nothing names the window yet, so the pane says 200k and marks it a guess.
    let app = app_with(vec![session.clone()]);
    let screen = render(&app, 140, 40);
    assert!(screen.contains("169k / 200k~"), "guess missing:\n{screen}");
    assert!(
        screen.contains("85%"),
        "guess percentage missing:\n{screen}"
    );

    // A cost-state line billed this model at [1m], so the guess is over.
    session.detail.long_context_models = vec!["claude-opus-5".into()];
    let app = app_with(vec![session.clone()]);
    let screen = render(&app, 140, 40);
    assert!(
        screen.contains("169k / 1.0M"),
        "recorded missing:\n{screen}"
    );
    assert!(
        !screen.contains("1.0M~"),
        "recorded still a guess:\n{screen}"
    );
    assert!(screen.contains("17%"), "recorded percentage:\n{screen}");

    // Settings alone carry the same weight once the family matches.
    session.detail.long_context_models.clear();
    session.configured_model = Some("opus[1m]".into());
    let app = app_with(vec![session.clone()]);
    let screen = render(&app, 140, 40);
    assert!(
        screen.contains("169k / 1.0M"),
        "configured missing:\n{screen}"
    );

    // ...but not for a session that moved to another model.
    session.detail.model = Some("claude-sonnet-5".into());
    let app = app_with(vec![session]);
    let screen = render(&app, 140, 40);
    assert!(
        screen.contains("169k / 200k~"),
        "sonnet wrongly widened:\n{screen}"
    );
}

/// The pane has to carry a session with no recap, which is most of them.
#[test]
fn the_activity_pane_shows_the_live_stream() {
    let mut app = app_with(vec![populated_session()]);
    app.tab = Tab::Activity;

    let screen = render(&app, 140, 40);
    assert!(screen.contains("Next: run the tests"), "recap missing:\n{screen}");
    assert!(
        screen.contains("pstack:poteto-mode"),
        "driver missing:\n{screen}"
    );
    assert!(screen.contains("135 messages"), "turn missing:\n{screen}");
    // Newest first, so the Bash call sits above the Edit that preceded it.
    let bash = screen.find("Run the test suite").expect("bash call");
    let edit = screen.find("Edit ").expect("edit call");
    assert!(bash < edit, "tools not newest first:\n{screen}");
    // The edited file is shown relative to the session's own directory.
    assert!(
        screen.contains("src/ui/activity.rs"),
        "file missing:\n{screen}"
    );

    let mut bare = populated_session();
    bare.detail.activity.recap = None;
    let mut app = app_with(vec![bare]);
    app.tab = Tab::Activity;
    let screen = render(&app, 140, 40);
    assert!(
        screen.contains("Run the test suite"),
        "pane empty without a recap:\n{screen}"
    );
}

#[test]
fn an_activity_free_session_says_so() {
    let mut session = populated_session();
    session.detail.activity = Activity::default();
    let mut app = app_with(vec![session]);
    app.tab = Tab::Activity;

    let screen = render(&app, 140, 40);
    assert!(screen.contains("Nothing recorded"), "hint missing:\n{screen}");
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
