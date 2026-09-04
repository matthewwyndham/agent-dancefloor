//! The Activity pane: what the session is doing, newest first.
//!
//! The recap leads because one sentence of prose beats a hundred tool names when
//! you are scanning a fleet. Under it sit the files the session changed, then
//! every tool call in the tail. The pane scrolls, so nothing is cut for length.
//!
//! Rows are built at a fixed width and never wrapped by the widget. That is what
//! makes scrolling exact: one row of content is one row of screen, so the offset
//! that shows the cursor can be computed rather than guessed.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Focus};
use crate::model::{duration_short, Activity, Session, ToolCall};
use crate::ui::{label_value, ACCENT, LABEL, SELECTED_BG};

/// Two spaces of indent, the tool-name column, and the space after it.
const ROW_INDENT: usize = 15;

/// One built row, and the tool it points at when it points at one.
struct Row {
    line: Line<'static>,
    tool: Option<usize>,
}

impl Row {
    fn plain(line: Line<'static>) -> Self {
        Row { line, tool: None }
    }
}

pub fn draw(frame: &mut Frame, app: &App, session: &Session, area: Rect) {
    let activity = &session.detail.activity;
    if activity.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "Nothing recorded in the transcript tail yet.",
                Style::new().fg(LABEL),
            ))
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    // The position readout is pinned. Letting it scroll with the list would
    // take away the one thing that says where in the list you are.
    let [status, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    if !activity.tools.is_empty() {
        frame.render_widget(Paragraph::new(status_line(app, activity)), status);
    }

    let width = body.width as usize;
    let height = body.height as usize;
    let rows = build_rows(session, width);

    // The cursor rides the middle of the pane, so the same key moves the same
    // distance whether the list is at its start, its end, or anywhere between.
    let focused = app.focus != Focus::Sessions;
    let anchor = focused
        .then(|| {
            rows.iter()
                .position(|row| row.tool == Some(app.tool_cursor))
        })
        .flatten();
    let offset = anchor
        .map(|row| row.saturating_sub(height / 2))
        .unwrap_or(0)
        .min(rows.len().saturating_sub(height));

    let visible: Vec<Line> = rows
        .into_iter()
        .skip(offset)
        .take(height)
        .map(|row| {
            if focused && row.tool == Some(app.tool_cursor) {
                highlight(row.line, width)
            } else {
                row.line
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(visible), body);
}

/// Where the cursor is in the list, or how long the list is when nothing is
/// browsing it yet.
fn status_line(app: &App, activity: &Activity) -> Line<'static> {
    let total = activity.tools.len();
    let mut spans = vec![
        Span::styled("tools", Style::new().fg(ACCENT).bold()),
        Span::styled(
            if app.focus == Focus::Sessions {
                format!("  {total}")
            } else {
                format!("  {} of {total}", app.tool_cursor + 1)
            },
            Style::new().fg(LABEL),
        ),
    ];
    if app.focus == Focus::Sessions {
        spans.push(Span::styled(
            "  enter to browse",
            Style::new().fg(Color::Gray),
        ));
    }
    Line::from(spans)
}

fn build_rows(session: &Session, width: usize) -> Vec<Row> {
    let activity = &session.detail.activity;
    let room = width.saturating_sub(ROW_INDENT);

    let mut rows = recap_rows(activity, width);
    rows.extend(turn_rows(activity));
    rows.extend(file_rows(session, width));
    rows.extend(tool_rows(activity, room));
    rows
}

fn recap_rows(activity: &Activity, width: usize) -> Vec<Row> {
    let Some(recap) = &activity.recap else {
        return Vec::new();
    };
    let mut rows: Vec<Row> = wrap(recap, width)
        .into_iter()
        .map(|text| Row::plain(Line::raw(text)))
        .collect();
    rows.push(Row::plain(Line::raw("")));
    rows
}

/// Who is steering, and how long the turn before this one took.
fn turn_rows(activity: &Activity) -> Vec<Row> {
    let mut rows = Vec::new();
    if let Some(driver) = &activity.driver {
        rows.push(Row::plain(label_value("driving", driver.label())));
    }
    if let Some(turn) = &activity.last_turn {
        rows.push(Row::plain(label_value(
            "last turn",
            format!(
                "{} · {} messages",
                duration_short(turn.duration_ms / 1000),
                turn.messages
            ),
        )));
    }
    if !rows.is_empty() {
        rows.push(Row::plain(Line::raw("")));
    }
    rows
}

/// Edited files, shown relative to the session's directory when they sit under
/// it. Plans and scratch files land elsewhere and keep their full path.
fn file_rows(session: &Session, width: usize) -> Vec<Row> {
    let files = &session.detail.activity.files;
    if files.is_empty() {
        return Vec::new();
    }
    let mut rows = vec![Row::plain(heading("files touched", files.len()))];
    for path in files.iter().rev() {
        let shown = path.strip_prefix(&session.cwd).unwrap_or(path);
        rows.push(Row::plain(Line::from(Span::styled(
            format!(
                "  {}",
                clip(&shown.to_string_lossy(), width.saturating_sub(2))
            ),
            Style::new().fg(Color::Gray),
        ))));
    }
    rows.push(Row::plain(Line::raw("")));
    rows
}

/// The list itself carries no heading: the pinned status row above the pane
/// names it, and repeating that would cost a row of the list.
fn tool_rows(activity: &Activity, room: usize) -> Vec<Row> {
    activity
        .tools
        .iter()
        .rev()
        .enumerate()
        .map(|(index, call)| Row {
            line: tool_line(call, room),
            tool: Some(index),
        })
        .collect()
}

/// The summary and the command both, because a written summary says why a call
/// ran and only the command itself says what it will do.
fn tool_line(call: &ToolCall, room: usize) -> Line<'static> {
    // The trailing space is not padding: a tool name wider than the column would
    // otherwise run straight into what follows it.
    let mut spans = vec![Span::styled(
        format!("  {:<12} ", call.name),
        Style::new().bold(),
    )];

    let summary = clip(&call.summary, room);
    let mut left = room;
    if !summary.is_empty() {
        left = left.saturating_sub(summary.chars().count() + 3);
        spans.push(Span::raw(summary));
        spans.push(Span::styled(" · ", Style::new().fg(LABEL)));
    }
    if !call.detail.is_empty() && left > 2 {
        spans.push(Span::styled(
            clip(&call.detail, left),
            Style::new().fg(Color::Gray),
        ));
    }
    Line::from(spans)
}

/// The whole command, for the one call the cursor is on. The pane has room for a
/// row of it; this has room for the rest.
pub fn draw_tool(frame: &mut Frame, app: &App, area: Rect) {
    let Some(call) = app.selected_tool() else {
        return;
    };

    let mut lines = vec![Line::from(Span::styled(
        call.name.clone(),
        Style::new().fg(ACCENT).bold(),
    ))];
    if !call.summary.is_empty() {
        lines.push(Line::raw(call.summary.clone()));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        if call.detail.is_empty() {
            "This call carried no text input.".to_string()
        } else {
            call.detail.clone()
        },
        Style::new().fg(Color::Gray),
    )));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        app.copy_notice
            .clone()
            .unwrap_or_else(|| "y to copy · esc to close".to_string()),
        Style::new().fg(LABEL),
    )));

    // Sized to the command it holds. A fixed height leaves a one-line call
    // sitting in an empty box.
    let width = 88.min(area.width.saturating_sub(4));
    let inner = width.saturating_sub(2) as usize;
    let content: usize = lines.iter().map(|line| wrapped_height(line, inner)).sum();
    let height = (content as u16 + 2).min(area.height.saturating_sub(2));
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
                    .title(" tool call ")
                    .border_style(Style::new().fg(ACCENT)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

/// How many rows a line takes once the popup wraps it.
fn wrapped_height(line: &Line, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    let length: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    length.div_ceil(width).max(1)
}

fn heading(text: &'static str, count: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled(text, Style::new().fg(ACCENT).bold()),
        Span::styled(format!("  {count}"), Style::new().fg(LABEL)),
    ])
}

/// Pad to the full width so the selection reads as a bar rather than as a patch
/// behind the text.
fn highlight(line: Line<'static>, width: usize) -> Line<'static> {
    let used: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    let mut line = line;
    if used < width {
        line.spans.push(Span::raw(" ".repeat(width - used)));
    }
    line.style(Style::new().bg(SELECTED_BG))
}

/// Greedy word wrap. The pane builds its own rows because a widget that wraps
/// for itself makes the scroll offset unknowable.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width < 2 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            rows.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        rows.push(line);
    }
    // A single word longer than the pane survives the loop intact, so every row
    // is cut to width on the way out.
    rows.into_iter().map(|row| clip(&row, width)).collect()
}

/// One row per call, so a target that would wrap is cut instead. A wrapped tool
/// list doubles in height and stops reading as a list.
///
/// A path is identified by its end, so it loses its front; everything else
/// loses its tail.
fn clip(text: &str, room: usize) -> String {
    let length = text.chars().count();
    if room < 2 || length <= room {
        return text.to_string();
    }
    if text.starts_with('/') {
        let dropped = length - room + 1;
        return std::iter::once('…')
            .chain(text.chars().skip(dropped))
            .collect();
    }
    text.chars().take(room - 1).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::{clip, wrap};

    #[test]
    fn a_path_keeps_the_end_and_prose_keeps_the_start() {
        assert_eq!(clip("/repo/src/ui/activity.rs", 12), "…activity.rs");
        assert_eq!(clip("run the whole suite", 8), "run the…");
        assert_eq!(clip("short", 12), "short");
    }

    #[test]
    fn wrapping_fills_each_row_and_cuts_an_unbreakable_word() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
        assert_eq!(wrap(&"x".repeat(20), 6), ["xxxxx…"]);
        assert!(wrap("anything", 1).is_empty());
    }
}
