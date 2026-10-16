//! Application state and the input handling that mutates it.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::digest::Step;
use crate::model::{ClientKind, Limits, Session, SessionKey, Subagent, ToolCall};
use crate::{clipboard, digest, process, providers, settings, subagents, transcript};

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
    /// The selected agent's thread, open in full. `esc` closes it.
    Thread,
}

/// One subagent's conversation, open on screen.
pub struct Thread {
    pub title: String,
    pub path: PathBuf,
    pub steps: Vec<Step>,
    /// Log size at the last read.
    bytes: u64,
    /// First row shown. `usize::MAX` follows the end.
    pub scroll: usize,
    /// Set by draw, which knows the width.
    pub max_scroll: Cell<usize>,
    pub page: Cell<usize>,
}

impl Thread {
    /// The row the draw shows first.
    pub fn top(&self) -> usize {
        self.scroll.min(self.max_scroll.get())
    }
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
    pub provider_homes: providers::ProviderHomes,
    pub sessions: Vec<Session>,
    pub selected: usize,
    pub tab: Tab,
    pub sort: Sort,
    pub focus: Focus,
    /// Which tool the Activity pane points at, newest first. It survives a
    /// refresh, so a list that grows under the cursor does not move it.
    pub tool_cursor: usize,
    /// Which agent the Agents pane points at.
    pub agent_cursor: usize,
    pub thread: Option<Thread>,
    /// What the last copy did. Shown in the open tool, cleared when it closes.
    pub copy_notice: Option<String>,
    /// A file to open in the editor.
    pub pending_open: Option<PathBuf>,
    /// Footer message; the next key clears it.
    pub notice: Option<String>,
    pub limits: Limits,
    pub interval: Duration,
    pub last_refresh: Instant,
    pub scan_error: Option<String>,
    pub provider_warnings: Vec<providers::ProviderWarning>,
    pub show_help: bool,
    pub should_quit: bool,
    /// Locating a transcript means scanning every project directory, so the
    /// answer is kept for the life of the session rather than re-derived.
    transcript_paths: HashMap<SessionKey, Option<PathBuf>>,
}

impl App {
    pub fn new(claude_home: PathBuf, interval: Duration, limits: Limits) -> Self {
        let mut provider_homes = providers::ProviderHomes::from_env();
        provider_homes.claude = claude_home.clone();
        Self {
            claude_home,
            provider_homes,
            sessions: Vec::new(),
            selected: 0,
            tab: Tab::Detail,
            sort: Sort::Status,
            focus: Focus::Sessions,
            tool_cursor: 0,
            agent_cursor: 0,
            thread: None,
            copy_notice: None,
            pending_open: None,
            notice: None,
            limits,
            interval,
            last_refresh: Instant::now(),
            scan_error: None,
            provider_warnings: Vec::new(),
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
        self.reload_thread();
        let anchor = self.selected_session().map(Session::key);
        let snapshot = process::snapshot();

        let mut sessions = match providers::claude::discover(&self.claude_home, &snapshot) {
            Ok(sessions) => {
                self.scan_error = None;
                sessions
            }
            Err(err) => {
                self.scan_error = Some(err.to_string());
                return;
            }
        };
        self.provider_warnings.clear();
        match providers::codex::discover(&self.provider_homes.codex, &snapshot) {
            Ok(mut found) => sessions.append(&mut found),
            Err(err) => self.provider_warnings.push(providers::ProviderWarning {
                provider: "Codex",
                message: err.to_string(),
            }),
        }
        match providers::pi::discover(&self.provider_homes.pi_sessions, &snapshot) {
            Ok(mut found) => sessions.append(&mut found),
            Err(err) => self.provider_warnings.push(providers::ProviderWarning {
                provider: "Pi",
                message: err.to_string(),
            }),
        }

        for session in &mut sessions {
            if session.client != ClientKind::Claude {
                continue;
            }
            let path = self
                .transcript_paths
                .entry(session.key())
                .or_insert_with(|| transcript::locate(&self.claude_home, &session.session_id))
                .clone();
            if let Some(path) = path {
                session.detail = transcript::read(&path);
                session.detail.subagents = subagents::read(&path);
            }
            // Re-read every tick, not cached: settings can change under a
            // running session, and three small files cost nothing next to the
            // transcript tail above.
            if session.client == ClientKind::Claude {
                session.configured_model = settings::model_for(&self.claude_home, &session.cwd);
            }
        }

        // Providers may observe more than one launcher/runtime process for a
        // single transcript. The UI identity is the provider-qualified
        // session key, so enforce the same invariant at the merge boundary.
        let mut seen = HashSet::new();
        sessions.retain(|session| seen.insert(session.key()));

        self.sessions = sessions;
        self.sort_sessions();
        self.prune_transcript_cache();

        self.selected = anchor
            .and_then(|key| self.sessions.iter().position(|s| s.key() == key))
            .unwrap_or(self.selected)
            .min(self.sessions.len().saturating_sub(1));

        // The tail moves under the cursor on every refresh, so a list that lost
        // entries must not leave the cursor pointing past the end of it.
        self.tool_cursor = self
            .tool_cursor
            .min(self.visible_tools().len().saturating_sub(1));
        self.agent_cursor = self
            .agent_cursor
            .min(self.visible_agents().len().saturating_sub(1));
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
        let live: Vec<SessionKey> = self.sessions.iter().map(Session::key).collect();
        self.transcript_paths.retain(|id, _| live.contains(id));
    }

    pub fn select_next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.sessions.len();
        self.tool_cursor = 0;
        self.agent_cursor = 0;
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
        self.agent_cursor = 0;
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

    pub fn visible_agents(&self) -> &[Subagent] {
        self.selected_session()
            .map(|session| session.detail.subagents.as_slice())
            .unwrap_or(&[])
    }

    pub fn selected_agent(&self) -> Option<&Subagent> {
        self.visible_agents().get(self.agent_cursor)
    }

    pub fn select_next_agent(&mut self) {
        let last = self.visible_agents().len().saturating_sub(1);
        self.agent_cursor = (self.agent_cursor + 1).min(last);
    }

    pub fn select_previous_agent(&mut self) {
        self.agent_cursor = self.agent_cursor.saturating_sub(1);
    }

    /// Move the cursor of the current tab.
    pub fn cursor_down(&mut self) {
        match self.tab {
            Tab::Agents => self.select_next_agent(),
            _ => self.select_next_tool(),
        }
    }

    pub fn cursor_up(&mut self) {
        match self.tab {
            Tab::Agents => self.select_previous_agent(),
            _ => self.select_previous_tool(),
        }
    }

    /// Open what the cursor points at.
    pub fn open_selected(&mut self) {
        match self.tab {
            Tab::Agents => self.open_thread(),
            _ => self.open_tool(),
        }
    }

    pub fn open_thread(&mut self) {
        let Some(agent) = self.selected_agent() else {
            return;
        };
        let Some(path) = agent.transcript.clone() else {
            self.notice = Some("no log for this agent".to_string());
            return;
        };
        let title = match agent.agent_type.is_empty() {
            true => agent.name.clone(),
            false => format!("{} · {}", agent.agent_type, agent.name),
        };
        let thread = match digest::thread(&path) {
            Ok(steps) => Thread {
                title,
                bytes: file_size(&path),
                path,
                steps,
                scroll: 0,
                max_scroll: Cell::new(0),
                page: Cell::new(1),
            },
            Err(error) => {
                self.notice = Some(format!("read failed: {error}"));
                return;
            }
        };
        self.thread = Some(thread);
        self.focus = Focus::Thread;
    }

    pub fn close_thread(&mut self) {
        self.thread = None;
        self.focus = Focus::Pane;
    }

    /// Re-read a live log that grew.
    fn reload_thread(&mut self) {
        let Some(thread) = self.thread.as_mut() else {
            return;
        };
        let bytes = file_size(&thread.path);
        if bytes == thread.bytes {
            return;
        }
        if let Ok(steps) = digest::thread(&thread.path) {
            thread.steps = steps;
            thread.bytes = bytes;
        }
    }

    /// Scroll by rows. The end follows growth.
    pub fn scroll_thread(&mut self, rows: isize) {
        let Some(thread) = self.thread.as_mut() else {
            return;
        };
        let max = thread.max_scroll.get();
        let next = thread.top().saturating_add_signed(rows).min(max);
        thread.scroll = if next == max { usize::MAX } else { next };
    }

    /// Scroll by a page, signed.
    pub fn page_thread(&mut self, pages: isize) {
        let page = self.thread.as_ref().map_or(1, |t| t.page.get().max(1));
        self.scroll_thread(pages * page as isize);
    }

    pub fn thread_home(&mut self) {
        if let Some(thread) = self.thread.as_mut() {
            thread.scroll = 0;
        }
    }

    pub fn thread_end(&mut self) {
        if let Some(thread) = self.thread.as_mut() {
            thread.scroll = usize::MAX;
        }
    }

    /// Step into the pane. A pane with nothing to point at still takes focus, so
    /// that `enter` and `esc` mean the same thing on every tab.
    pub fn focus_pane(&mut self) {
        self.focus = Focus::Pane;
        self.tool_cursor = self
            .tool_cursor
            .min(self.visible_tools().len().saturating_sub(1));
        self.agent_cursor = self
            .agent_cursor
            .min(self.visible_agents().len().saturating_sub(1));
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

    /// Queue the raw log for the editor.
    pub fn open_raw_log(&mut self) {
        if let Some(thread) = &self.thread {
            self.pending_open = Some(thread.path.clone());
            return;
        }
        match self
            .selected_session()
            .and_then(|s| s.detail.transcript.clone())
        {
            Some(path) => self.pending_open = Some(path),
            None => self.notice = Some("no log for this session".to_string()),
        }
    }

    /// Write the digest, then queue it.
    pub fn open_digest(&mut self) {
        let Some(session) = self.selected_session() else {
            self.notice = Some("no session selected".to_string());
            return;
        };
        match digest::write(session) {
            Ok(path) => self.pending_open = Some(path),
            Err(error) => self.notice = Some(format!("digest failed: {error}")),
        }
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

fn file_size(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}
