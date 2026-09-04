//! Application state and the input handling that mutates it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::model::{Limits, Session, ToolCall};
use crate::{clipboard, discovery, settings, subagents, transcript};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Detail,
    Agents,
    Prompt,
    Usage,
    Activity,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Detail,
        Tab::Agents,
        Tab::Prompt,
        Tab::Usage,
        Tab::Activity,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Detail => "Detail",
            Tab::Agents => "Agents",
            Tab::Prompt => "Prompt",
            Tab::Usage => "Usage",
            Tab::Activity => "Activity",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// Which half of the screen the keys act on, and whether a tool is open on top
/// of it. Nesting the states means arrows cannot move two things at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Arrows move between sessions. `enter` steps into the pane.
    Sessions,
    /// Arrows move the cursor inside the pane. `esc` steps back out.
    Pane,
    /// The selected tool call, open in full. `esc` closes it.
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Status,
    Context,
    Uptime,
    Directory,
}

impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Sort::Status => "status",
            Sort::Context => "context",
            Sort::Uptime => "uptime",
            Sort::Directory => "dir",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Sort::Status => Sort::Context,
            Sort::Context => Sort::Uptime,
            Sort::Uptime => Sort::Directory,
            Sort::Directory => Sort::Status,
        }
    }
}

pub struct App {
    pub claude_home: PathBuf,
    pub sessions: Vec<Session>,
    pub selected: usize,
    pub tab: Tab,
    pub sort: Sort,
    pub focus: Focus,
    /// Which tool the Activity pane points at, newest first. It survives a
    /// refresh, so a list that grows under the cursor does not move it.
    pub tool_cursor: usize,
    /// What the last copy did. Shown in the open tool, cleared when it closes.
    pub copy_notice: Option<String>,
    pub limits: Limits,
    pub interval: Duration,
    pub last_refresh: Instant,
    pub scan_error: Option<String>,
    pub show_help: bool,
    pub should_quit: bool,
    /// Locating a transcript means scanning every project directory, so the
    /// answer is kept for the life of the session rather than re-derived.
    transcript_paths: HashMap<String, Option<PathBuf>>,
}

impl App {
    pub fn new(claude_home: PathBuf, interval: Duration, limits: Limits) -> Self {
        Self {
            claude_home,
            sessions: Vec::new(),
            selected: 0,
            tab: Tab::Detail,
            sort: Sort::Status,
            focus: Focus::Sessions,
            tool_cursor: 0,
            copy_notice: None,
            limits,
            interval,
            last_refresh: Instant::now(),
            scan_error: None,
            show_help: false,
            should_quit: false,
            transcript_paths: HashMap::new(),
        }
    }

    pub fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected)
    }

    /// Rebuild the whole session list. Selection follows the session that was
    /// highlighted, because sorting can move rows under the cursor.
    pub fn refresh(&mut self) {
        self.last_refresh = Instant::now();
        let anchor = self.selected_session().map(|s| s.pid);

        let mut sessions = match discovery::scan(&self.claude_home) {
            Ok(sessions) => {
                self.scan_error = None;
                sessions
            }
            Err(err) => {
                self.scan_error = Some(err.to_string());
                return;
            }
        };

        for session in &mut sessions {
            let path = self
                .transcript_paths
                .entry(session.session_id.clone())
                .or_insert_with(|| transcript::locate(&self.claude_home, &session.session_id))
                .clone();
            if let Some(path) = path {
                session.detail = transcript::read(&path);
                session.detail.subagents = subagents::read(&path);
            }
            // Re-read every tick, not cached: settings can change under a
            // running session, and three small files cost nothing next to the
            // transcript tail above.
            session.configured_model = settings::model_for(&self.claude_home, &session.cwd);
        }

        self.sessions = sessions;
        self.sort_sessions();
        self.prune_transcript_cache();

        self.selected = anchor
            .and_then(|pid| self.sessions.iter().position(|s| s.pid == pid))
            .unwrap_or(self.selected)
            .min(self.sessions.len().saturating_sub(1));

        // The tail moves under the cursor on every refresh, so a list that lost
        // entries must not leave the cursor pointing past the end of it.
        self.tool_cursor = self
            .tool_cursor
            .min(self.visible_tools().len().saturating_sub(1));
    }

    fn sort_sessions(&mut self) {
        let limits = self.limits;
        let now = Self::now_ms();
        match self.sort {
            // Waiting first, then busy, then the name: the sessions that need a
            // human stay on top, and the ordering is stable between refreshes.
            Sort::Status => {
                self.sessions
                    .sort_by(|a, b| match (a.status as u8).cmp(&(b.status as u8)) {
                        std::cmp::Ordering::Equal => {
                            a.name.to_lowercase().cmp(&b.name.to_lowercase())
                        }
                        other => other,
                    })
            }
            Sort::Context => self.sessions.sort_by(|a, b| {
                b.context_ratio(limits)
                    .partial_cmp(&a.context_ratio(limits))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            Sort::Uptime => self
                .sessions
                .sort_by_key(|session| std::cmp::Reverse(session.uptime_secs(now))),
            Sort::Directory => self.sessions.sort_by(|a, b| {
                a.dir_label()
                    .to_lowercase()
                    .cmp(&b.dir_label().to_lowercase())
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }),
        }
    }

    /// Drop cached paths for sessions that have exited, so the map cannot grow
    /// for as long as the process runs.
    fn prune_transcript_cache(&mut self) {
        if self.transcript_paths.len() <= self.sessions.len() {
            return;
        }
        let live: Vec<String> = self.sessions.iter().map(|s| s.session_id.clone()).collect();
        self.transcript_paths.retain(|id, _| live.contains(id));
    }

    pub fn select_next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.sessions.len();
        self.tool_cursor = 0;
    }

    pub fn select_previous(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.sessions.len() - 1
        } else {
            self.selected - 1
        };
        self.tool_cursor = 0;
    }

    /// The tools the Activity pane lists, newest first.
    pub fn visible_tools(&self) -> &[ToolCall] {
        self.selected_session()
            .map(|session| session.detail.activity.tools.as_slice())
            .unwrap_or(&[])
    }

    pub fn selected_tool(&self) -> Option<&ToolCall> {
        let tools = self.visible_tools();
        tools.get(tools.len().checked_sub(self.tool_cursor + 1)?)
    }

    /// Down the list is back in time, because the newest call is at the top.
    pub fn select_next_tool(&mut self) {
        let last = self.visible_tools().len().saturating_sub(1);
        self.tool_cursor = (self.tool_cursor + 1).min(last);
    }

    pub fn select_previous_tool(&mut self) {
        self.tool_cursor = self.tool_cursor.saturating_sub(1);
    }

    /// Step into the pane. A pane with nothing to point at still takes focus, so
    /// that `enter` and `esc` mean the same thing on every tab.
    pub fn focus_pane(&mut self) {
        self.focus = Focus::Pane;
        self.tool_cursor = self.tool_cursor.min(self.visible_tools().len().saturating_sub(1));
    }

    pub fn focus_sessions(&mut self) {
        self.focus = Focus::Sessions;
    }

    pub fn open_tool(&mut self) {
        if self.selected_tool().is_some() {
            self.copy_notice = None;
            self.focus = Focus::Tool;
        }
    }

    pub fn close_tool(&mut self) {
        self.copy_notice = None;
        self.focus = Focus::Pane;
    }

    /// The command is copied whole, not the row the pane had room for.
    pub fn copy_tool(&mut self) {
        let Some(tool) = self.selected_tool() else {
            return;
        };
        let text = if tool.detail.is_empty() {
            tool.summary.clone()
        } else {
            tool.detail.clone()
        };
        if text.is_empty() {
            self.copy_notice = Some("nothing to copy".to_string());
            return;
        }
        self.copy_notice = Some(match clipboard::copy(&text) {
            Some(error) => format!("copy failed: {error}"),
            None => format!("copied {} characters", text.chars().count()),
        });
    }

    pub fn next_tab(&mut self) {
        let index = (self.tab.index() + 1) % Tab::ALL.len();
        self.tab = Tab::ALL[index];
    }

    pub fn previous_tab(&mut self) {
        let index = (self.tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len();
        self.tab = Tab::ALL[index];
    }

    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        self.sort_sessions();
    }
}
