//! Screen layout: a header, the session list beside the detail pane, a footer.

mod activity;
mod detail;
mod list;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Focus};

pub const ACCENT: Color = Color::Cyan;
pub const LABEL: Color = Color::DarkGray;
/// The row a cursor sits on, in both the session list and the Activity pane.
pub const SELECTED_BG: Color = Color::Indexed(238);

/// The focused half of the screen is the one with the coloured border.
pub fn border_color(focused: bool) -> Color {
    if focused {
        ACCENT
    } else {
        LABEL
    }
}

/// Context colour, shared by the list column and the detail gauge so one
/// session never reads as two different severities.
pub fn context_color(ratio: f64) -> Color {
    if ratio >= 0.85 {
        Color::Red
    } else if ratio >= 0.60 {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// Status colour, shared by the list glyph and the detail heading. Waiting is
/// yellow so the one state that needs a human stands apart from the two that
/// do not.
pub fn status_color(status: crate::model::Status) -> Color {
    match status {
        crate::model::Status::Waiting => Color::Yellow,
        crate::model::Status::Busy => Color::Green,
        crate::model::Status::Idle => Color::Blue,
        crate::model::Status::Other => Color::DarkGray,
    }
}

pub fn label_value<'a>(label: &'a str, value: impl Into<String>) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{label:<12}"), Style::new().fg(LABEL)),
        Span::raw(value.into()),
    ])
}

pub fn draw(frame: &mut Frame, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, header);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(38), Constraint::Min(0)]).areas(body);
    list::draw(frame, app, left);
    detail::draw(frame, app, right);

    draw_footer(frame, app, footer);

    if app.focus == Focus::Tool {
        activity::draw_tool(frame, app, frame.area());
    }
    if app.show_help {
        draw_help(frame, frame.area());
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let busy = app
        .sessions
        .iter()
        .filter(|s| s.status == crate::model::Status::Busy)
        .count();
    let waiting = app
        .sessions
        .iter()
        .filter(|s| s.status == crate::model::Status::Waiting)
        .count();

    let mut spans = vec![
        Span::styled(" dancefloor ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::raw(" "),
        Span::styled(
            format!("{} session{}", app.sessions.len(), plural(app.sessions.len())),
            Style::new().bold(),
        ),
        Span::styled(" · ", Style::new().fg(LABEL)),
        Span::styled(format!("{busy} busy"), Style::new().fg(Color::Green)),
        Span::styled(" · ", Style::new().fg(LABEL)),
        // Reversed rather than merely coloured: across a header of grey text,
        // a filled block is what the eye lands on first.
        Span::styled(
            format!(" {} {waiting} waiting ", crate::model::Status::Waiting.glyph()),
            if waiting > 0 {
                Style::new().fg(Color::Black).bg(Color::Yellow).bold()
            } else {
                Style::new().fg(LABEL)
            },
        ),
        Span::styled(" · ", Style::new().fg(LABEL)),
        Span::styled(format!("sort {}", app.sort.label()), Style::new().fg(LABEL)),
        Span::styled(" · ", Style::new().fg(LABEL)),
        Span::styled(
            format!("every {}s", app.interval.as_secs().max(1)),
            Style::new().fg(LABEL),
        ),
    ];

    // A failed scan must be visible without opening a pane; it means the whole
    // list is stale, not just one row.
    if let Some(error) = &app.scan_error {
        spans.push(Span::styled(
            format!("  scan failed: {error}"),
            Style::new().fg(Color::Red).bold(),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    // The keys change with the focus, so the footer says what the arrows do
    // right now rather than what they do most of the time.
    let keys: &[(&str, &str)] = match app.focus {
        Focus::Sessions => &[
            ("j/k", "move"),
            ("enter", "focus pane"),
            ("tab", "pane"),
            ("1-5", "jump"),
            ("s", "sort"),
            ("?", "help"),
            ("q", "quit"),
        ],
        Focus::Pane => &[
            ("↑/↓", "scroll"),
            ("enter", "open call"),
            ("esc", "sessions"),
            ("tab", "pane"),
            ("1-5", "jump"),
            ("q", "quit"),
        ],
        Focus::Tool => &[("y", "copy"), ("esc", "close")],
    };
    let mut spans = Vec::new();
    for (key, action) in keys {
        spans.push(Span::styled(format!(" {key} "), Style::new().fg(ACCENT)));
        spans.push(Span::styled(*action, Style::new().fg(LABEL)));
    }
    if app.sessions.is_empty() {
        spans.push(Span::styled(
            "   no live sessions",
            Style::new().fg(Color::Yellow),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled("dancefloor", Style::new().fg(ACCENT).bold())),
        Line::raw(""),
        label_value("j / ↓", "next session"),
        label_value("k / ↑", "previous session"),
        label_value("enter", "focus the pane, then open a tool call"),
        label_value("esc", "back to the session list"),
        label_value("tab / l", "next pane"),
        label_value("shift-tab", "previous pane"),
        label_value("1 - 5", "Detail / Agents / Prompt / Usage / Activity"),
        label_value("s", "cycle sort order"),
        label_value("r", "refresh now"),
        label_value("y", "copy an open tool call"),
        label_value("? ", "close this help"),
        label_value("q", "quit"),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                format!("{:<12}", crate::model::Status::Waiting.glyph()),
                Style::new().fg(Color::Yellow).bold(),
            ),
            Span::raw("waiting for your input"),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{:<12}", crate::model::Status::Busy.glyph()),
                Style::new().fg(Color::Green),
            ),
            Span::raw("working"),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{:<12}", crate::model::Status::Idle.glyph()),
                Style::new().fg(Color::Blue),
            ),
            Span::raw("idle"),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "Context is read from the transcript's newest usage block.",
            Style::new().fg(LABEL),
        )),
        Line::from(Span::styled(
            "A ~ on the limit means it was inferred; set --context-limit to pin it.",
            Style::new().fg(LABEL),
        )),
    ];

    let width = 64.min(area.width.saturating_sub(4));
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::bordered()
                    .title(" help ")
                    .border_style(Style::new().fg(ACCENT)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}
