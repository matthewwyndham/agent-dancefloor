//! An open subagent thread, drawn over the screen.
//!
//! Rows are built at the popup width, so one row of content is one row of
//! screen and the scroll offset is exact.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::digest::Step;
use crate::ui::activity::wrap;
use crate::ui::{ACCENT, LABEL};

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let Some(thread) = &app.thread else {
        return;
    };

    let popup = Rect {
        x: area.x + 2.min(area.width / 8),
        y: area.y + 1.min(area.height / 8),
        width: area.width.saturating_sub(2 * 2.min(area.width / 8)),
        height: area.height.saturating_sub(2 * 1.min(area.height / 8)),
    };
    let block = Block::bordered()
        .title(format!(" {} ", thread.title))
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    if inner.height < 2 {
        return;
    }

    // The last row holds the keys.
    let height = (inner.height - 1) as usize;
    let width = inner.width as usize;
    let rows = build_rows(&thread.steps, width);
    let total = rows.len();

    thread.max_scroll.set(rows.len().saturating_sub(height));
    thread.page.set(height.saturating_sub(1).max(1));
    let top = thread.top();

    let visible: Vec<Line> = rows.into_iter().skip(top).take(height).collect();
    let shown = visible.len();
    frame.render_widget(
        Paragraph::new(visible),
        Rect {
            height: height as u16,
            ..inner
        },
    );

    let keys = Line::from(vec![
        Span::styled(
            format!("{}-{} of {}  ", top + 1, top + shown, total),
            Style::new().fg(LABEL),
        ),
        Span::styled(
            "j/k scroll · space page · g/G ends · o raw log · esc close",
            Style::new().fg(LABEL),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(keys),
        Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        },
    );
}

fn build_rows(steps: &[Step], width: usize) -> Vec<Line<'static>> {
    if steps.is_empty() {
        return vec![Line::from(Span::styled(
            "No steps recorded yet.",
            Style::new().fg(LABEL),
        ))];
    }
    let mut rows = Vec::new();
    for step in steps {
        match step {
            Step::Prompt(text) => {
                rows.push(heading("prompt", ACCENT));
                prose(&mut rows, text, width, Style::new());
            }
            Step::Thinking(text) => {
                rows.push(heading("thinking", Color::Magenta));
                prose(
                    &mut rows,
                    text,
                    width,
                    Style::new().fg(Color::Gray).italic(),
                );
            }
            Step::Reply(text) => {
                rows.push(heading("reply", Color::Green));
                prose(&mut rows, text, width, Style::new());
            }
            Step::Call {
                tool,
                summary,
                input,
            } => {
                let mut spans = vec![Span::styled(format!("▸ {tool}"), Style::new().bold())];
                if !summary.is_empty() {
                    spans.push(Span::styled(
                        format!("  {summary}"),
                        Style::new().fg(Color::Gray),
                    ));
                }
                rows.push(Line::from(spans));
                code(&mut rows, input, width, Style::new().fg(Color::Gray));
            }
        }
        rows.push(Line::raw(""));
    }
    rows
}

fn heading(text: &'static str, color: Color) -> Line<'static> {
    Line::from(Span::styled(text, Style::new().fg(color).bold()))
}

/// Word wrap each line; keep blank lines.
fn prose(rows: &mut Vec<Line<'static>>, text: &str, width: usize, style: Style) {
    let room = width.saturating_sub(2);
    for line in text.lines() {
        let wrapped = wrap(line, room);
        if wrapped.is_empty() {
            rows.push(Line::raw(""));
        }
        for row in wrapped {
            rows.push(Line::from(Span::styled(format!("  {row}"), style)));
        }
    }
}

/// Cut lines at the width; keep indents.
fn code(rows: &mut Vec<Line<'static>>, text: &str, width: usize, style: Style) {
    let room = width.saturating_sub(2).max(1);
    for line in text.lines() {
        let chars: Vec<char> = line.replace('\t', "    ").chars().collect();
        if chars.is_empty() {
            rows.push(Line::raw(""));
        }
        for chunk in chars.chunks(room) {
            let row: String = chunk.iter().collect();
            rows.push(Line::from(Span::styled(format!("  {row}"), style)));
        }
    }
}
