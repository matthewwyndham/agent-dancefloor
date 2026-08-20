//! The right-hand pane: four views over the selected session.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use crate::app::{App, Tab};
use crate::model::{duration_short, tokens_short, Session};
use crate::ui::{context_color, label_value, ACCENT, LABEL};

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().border_style(Style::new().fg(LABEL));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [tabs_area, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            Line::from(vec![
                Span::styled(format!("{} ", index + 1), Style::new().fg(LABEL)),
                Span::raw(tab.title()),
            ])
        })
        .collect();
    frame.render_widget(
        Tabs::new(titles)
            .select(app.tab.index())
            .highlight_style(Style::new().fg(ACCENT).bold())
            .divider(Span::styled(" │ ", Style::new().fg(LABEL))),
        tabs_area,
    );

    let Some(session) = app.selected_session() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "Nothing selected.",
                Style::new().fg(LABEL),
            )),
            content,
        );
        return;
    };

    match app.tab {
        Tab::Detail => draw_detail(frame, app, session, content),
        Tab::Agents => draw_agents(frame, session, content),
        Tab::Prompt => draw_prompt(frame, session, content),
        Tab::Usage => draw_usage(frame, app, session, content),
    }
}

fn draw_detail(frame: &mut Frame, app: &App, session: &Session, area: Rect) {
    let [info, gauge] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(area);

    let mut lines = heading_lines(session);
    lines.extend(location_lines(session));
    lines.extend(runtime_lines(session));
    lines.extend(problem_lines(&session.detail));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), info);
    draw_context_gauge(frame, app, session, gauge);
}

/// Name, status, and how long the status has held.
fn heading_lines(session: &Session) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled(session.name.clone(), Style::new().fg(ACCENT).bold()),
            Span::styled(
                format!("  {} {}", session.status.glyph(), session.status.label()),
                Style::new().fg(LABEL),
            ),
            Span::styled(
                session
                    .status_age_secs(App::now_ms())
                    .map(|s| format!(" for {}", duration_short(s)))
                    .unwrap_or_default(),
                Style::new().fg(LABEL),
            ),
        ]),
        Line::raw(""),
    ]
}

/// Where the session works and what it is working on.
fn location_lines(session: &Session) -> Vec<Line<'static>> {
    let detail = &session.detail;
    let mut lines = Vec::new();

    if let Some(title) = &detail.title {
        lines.push(label_value("title", title.clone()));
    }
    lines.push(label_value("cwd", session.cwd.to_string_lossy().to_string()));
    lines.push(label_value(
        "branch",
        detail.git_branch.clone().unwrap_or_else(|| "—".into()),
    ));

    if let Some(worktree) = &detail.worktree {
        lines.push(label_value(
            "worktree",
            format!("{} (from {})", worktree.name, worktree.original_branch),
        ));
        lines.push(label_value("wt branch", worktree.branch.clone()));
        lines.push(label_value("wt path", worktree.path.clone()));
    }
    if let Some(pr) = &detail.pull_request {
        lines.push(label_value(
            "pr",
            format!("#{} {}", pr.number, pr.repository),
        ));
        lines.push(label_value("pr url", pr.url.clone()));
    }

    lines.push(label_value(
        "model",
        detail.model.clone().unwrap_or_else(|| "—".into()),
    ));
    lines.push(label_value(
        "mode",
        format!(
            "{} · perms {}",
            detail.mode.clone().unwrap_or_else(|| "—".into()),
            detail.permission_mode.clone().unwrap_or_else(|| "—".into()),
        ),
    ));
    if let Some(effort) = &detail.effort {
        lines.push(label_value("effort", effort.clone()));
    }
    lines
}

/// How long it has run, and what it costs to run.
fn runtime_lines(session: &Session) -> Vec<Line<'static>> {
    let detail = &session.detail;
    let mut lines = vec![
        Line::raw(""),
        label_value("uptime", duration_short(session.uptime_secs(App::now_ms()))),
        label_value(
            "last write",
            detail
                .transcript_age_secs
                .map(|s| format!("{} ago", duration_short(s)))
                .unwrap_or_else(|| "—".into()),
        ),
    ];

    if let Some(proc) = &session.proc {
        lines.push(label_value(
            "process",
            format!(
                "pid {} · cpu {:.1}% · rss {} MiB",
                session.pid,
                proc.cpu_percent,
                proc.rss_kib / 1024
            ),
        ));
    }
    lines.push(label_value(
        "client",
        format!(
            "v{} · {} · {}",
            session.version, session.kind, session.entrypoint
        ),
    ));
    lines.push(label_value(
        "agents",
        format!("{} spawned", detail.subagents.len()),
    ));
    lines
}

/// Says so when the transcript could not be read, rather than showing a pane
/// that looks merely empty.
fn problem_lines(detail: &crate::model::Detail) -> Vec<Line<'static>> {
    if let Some(error) = &detail.read_error {
        return vec![
            Line::raw(""),
            Line::from(Span::styled(
                format!("transcript unreadable: {error}"),
                Style::new().fg(Color::Red),
            )),
        ];
    }
    if detail.transcript.is_none() {
        return vec![
            Line::raw(""),
            Line::from(Span::styled(
                "no transcript found for this session",
                Style::new().fg(Color::Yellow),
            )),
        ];
    }
    Vec::new()
}

fn draw_context_gauge(frame: &mut Frame, app: &App, session: &Session, area: Rect) {
    let used = session
        .detail
        .usage
        .as_ref()
        .map(|u| u.total())
        .unwrap_or(0);
    let (limit, inferred) = session.context_limit(app.context_limit);
    let ratio = session.context_ratio(app.context_limit);

    // The counts sit in the title and only the percentage rides the bar: a long
    // label centred over a partly-filled gauge is unreadable at either end.
    let title = Line::from(vec![
        Span::styled(" context  ", Style::new().fg(LABEL)),
        Span::styled(tokens_short(used), Style::new().fg(context_color(ratio))),
        Span::styled(
            format!(" / {}{}", tokens_short(limit), if inferred { "~" } else { "" }),
            Style::new().fg(LABEL),
        ),
    ]);

    frame.render_widget(
        Gauge::default()
            .block(Block::default().title(title))
            .gauge_style(Style::new().fg(context_color(ratio)))
            .ratio(ratio)
            .label(format!("{:.0}%", ratio * 100.0)),
        area,
    );
}

fn draw_agents(frame: &mut Frame, session: &Session, area: Rect) {
    let agents = &session.detail.subagents;
    if agents.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "This session has spawned no subagents.",
                Style::new().fg(LABEL),
            )),
            area,
        );
        return;
    }

    let mut lines = Vec::new();
    for agent in agents {
        lines.push(Line::from(vec![
            Span::styled(agent.name.clone(), Style::new().bold()),
            Span::styled(
                format!("  {}", agent.agent_type),
                Style::new().fg(ACCENT),
            ),
            Span::styled(
                format!("  depth {}", agent.spawn_depth),
                Style::new().fg(LABEL),
            ),
        ]));
        // The description is the prompt or skill the agent was spawned for, so it
        // is the line that says what the agent is actually doing.
        if !agent.description.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  {}", agent.description),
                Style::new().fg(Color::Gray),
            )));
        }
        lines.push(Line::from(Span::styled(
            format!(
                "  {} KiB · idle {}",
                agent.bytes / 1024,
                agent
                    .age_secs
                    .map(duration_short)
                    .unwrap_or_else(|| "—".into())
            ),
            Style::new().fg(LABEL),
        )));
        lines.push(Line::raw(""));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_prompt(frame: &mut Frame, session: &Session, area: Rect) {
    let mut lines = Vec::new();
    if let Some(title) = &session.detail.title {
        lines.push(Line::from(Span::styled(
            title.clone(),
            Style::new().fg(ACCENT).bold(),
        )));
        lines.push(Line::raw(""));
    }
    match &session.detail.last_prompt {
        Some(prompt) => {
            for line in prompt.lines() {
                lines.push(Line::raw(line.to_string()));
            }
        }
        None => lines.push(Line::from(Span::styled(
            "No user prompt found in the transcript tail.",
            Style::new().fg(LABEL),
        ))),
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_usage(frame: &mut Frame, app: &App, session: &Session, area: Rect) {
    let [breakdown, gauge] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(area);

    let detail = &session.detail;
    let mut lines = vec![Line::from(Span::styled(
        "newest request",
        Style::new().fg(ACCENT).bold(),
    ))];

    match &detail.usage {
        Some(usage) => {
            lines.push(label_value("input", tokens_short(usage.input)));
            lines.push(label_value("cache read", tokens_short(usage.cache_read)));
            lines.push(label_value("cache write", tokens_short(usage.cache_creation)));
            lines.push(label_value("output", tokens_short(usage.output)));
            lines.push(label_value("total", tokens_short(usage.total())));
            lines.push(label_value("peak seen", tokens_short(detail.usage_peak)));
        }
        None => lines.push(Line::from(Span::styled(
            "no usage recorded yet",
            Style::new().fg(LABEL),
        ))),
    }

    let totals = &detail.totals;
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "recent activity",
        Style::new().fg(ACCENT).bold(),
    )));
    // These sum the parsed tail, not the session, so the pane must say so.
    lines.push(Line::from(Span::styled(
        "(over the transcript tail, not the whole session)",
        Style::new().fg(LABEL),
    )));
    lines.push(label_value(
        "messages",
        format!(
            "{} assistant · {} user",
            totals.assistant_messages, totals.user_messages
        ),
    ));
    lines.push(label_value("output", tokens_short(totals.output_tokens)));
    lines.push(label_value("thinking", tokens_short(totals.thinking_tokens)));
    lines.push(label_value(
        "cache write",
        tokens_short(totals.cache_creation_tokens),
    ));
    lines.push(label_value("web search", totals.web_searches.to_string()));
    if let Some(tier) = &detail.service_tier {
        lines.push(label_value("tier", tier.clone()));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        breakdown,
    );
    draw_context_gauge(frame, app, session, gauge);
}
