//! The session list. One row per live session, ordered by the active sort.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::model::{Session, Status};
use crate::ui::{context_color, ACCENT, LABEL};

const BAR_CELLS: u16 = 6;
const PERCENT_CELLS: u16 = 4;
const DIR_CELLS_MAX: u16 = 14;
/// Status glyph, the four single-column gaps a five-column table inserts, the
/// bar, and the percentage. Whatever is left is split between name and dir.
const FIXED_CELLS: u16 = 1 + 4 + BAR_CELLS + PERCENT_CELLS;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .title(Span::styled(" Sessions ", Style::new().fg(ACCENT)))
        .border_style(Style::new().fg(LABEL));

    if app.sessions.is_empty() {
        // Wrapped, because this pane is narrow by design and an unwrapped hint
        // is clipped mid-word at ordinary terminal widths.
        let hint = Paragraph::new(vec![
            Line::raw(""),
            Line::from(Span::styled(
                " No live sessions.",
                Style::new().fg(Color::Yellow),
            )),
            Line::raw(""),
            Line::from(Span::styled(
                " Start Claude Code anywhere and it shows up here.",
                Style::new().fg(LABEL),
            )),
        ])
        .block(block)
        .wrap(Wrap { trim: false });
        frame.render_widget(hint, area);
        return;
    }

    // Widths are computed here rather than left to the table so both text columns
    // can mark their own truncation. The name is the column that identifies the
    // session, so it takes the larger share.
    let flexible = area.width.saturating_sub(2).saturating_sub(FIXED_CELLS);
    let dir_width = DIR_CELLS_MAX.min(flexible / 3);
    let name_width = flexible.saturating_sub(dir_width);

    let rows: Vec<Row> = app
        .sessions
        .iter()
        .map(|session| build_row(session, app.context_limit, name_width, dir_width))
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(name_width),
            Constraint::Length(dir_width),
            Constraint::Length(BAR_CELLS),
            Constraint::Length(PERCENT_CELLS),
        ],
    )
    .block(block)
    .row_highlight_style(Style::new().bg(Color::Indexed(238)).bold())
    .highlight_symbol("");

    let mut state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn build_row<'a>(
    session: &'a Session,
    context_limit: Option<u64>,
    name_width: u16,
    dir_width: u16,
) -> Row<'a> {
    let ratio = session.context_ratio(context_limit);
    let color = context_color(ratio);

    Row::new(vec![
        Cell::from(Span::styled(
            session.status.glyph(),
            Style::new().fg(status_color(session.status)),
        )),
        Cell::from(Span::styled(
            elide(&session.name, name_width as usize),
            name_style(session.status),
        )),
        Cell::from(Span::styled(
            elide(&session.dir_label(), dir_width as usize),
            Style::new().fg(LABEL),
        )),
        Cell::from(Span::styled(bar(ratio), Style::new().fg(color))),
        Cell::from(Span::styled(
            format!("{:>3.0}%", ratio * 100.0),
            Style::new().fg(color),
        )),
    ])
}

fn name_style(status: Status) -> Style {
    match status {
        Status::Busy => Style::new().bold(),
        _ => Style::new(),
    }
}

fn status_color(status: Status) -> Color {
    match status {
        Status::Busy => Color::Green,
        Status::Idle => Color::Blue,
        Status::Other => Color::DarkGray,
    }
}

/// Shorten to `width`, marking the cut. A silently clipped directory name reads
/// as a different directory, which is the mistake worth spending a character on.
fn elide(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A fixed-width block bar. Rounds up so any non-zero usage shows at least one
/// filled cell — a session with context in it should never look empty.
fn bar(ratio: f64) -> String {
    let cells = BAR_CELLS as usize;
    let filled = ((ratio * cells as f64).ceil() as usize).min(cells);
    let mut out = String::with_capacity(cells * 3);
    for cell in 0..cells {
        out.push(if cell < filled { '█' } else { '░' });
    }
    out
}
