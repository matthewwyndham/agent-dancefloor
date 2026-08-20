//! Screen layout: a header, the session list beside the detail pane, a footer.

mod detail;
mod list;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

pub const ACCENT: Color = Color::Cyan;
pub const LABEL: Color = Color::DarkGray;

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
    let keys = [
        ("j/k", "move"),
        ("tab", "pane"),
        ("1-4", "jump"),
        ("s", "sort"),
        ("r", "refresh"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut spans = Vec::new();
    for (key, action) in keys {
        spans.push(Span::styled(format!(" {key} "), Style::new().fg(ACCENT)));
        spans.push(Span::styled(action, Style::new().fg(LABEL)));
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
        label_value("tab / l", "next pane"),
        label_value("shift-tab", "previous pane"),
        label_value("1 2 3 4", "Detail / Agents / Prompt / Usage"),
        label_value("s", "cycle sort order"),
        label_value("r", "refresh now"),
        label_value("? ", "close this help"),
        label_value("q / esc", "quit"),
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
