//! Application state and the input handling that mutates it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::model::Session;
use crate::{discovery, subagents, transcript};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Detail,
    Agents,
    Prompt,
    Usage,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Detail, Tab::Agents, Tab::Prompt, Tab::Usage];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Detail => "Detail",
            Tab::Agents => "Agents",
            Tab::Prompt => "Prompt",
            Tab::Usage => "Usage",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
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
    pub sessions: Vec<Session>,
    pub selected: usize,
    pub tab: Tab,
    pub sort: Sort,
    pub context_limit: Option<u64>,
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
    pub fn new(claude_home: PathBuf, interval: Duration, context_limit: Option<u64>) -> Self {
        Self {
            claude_home,
            sessions: Vec::new(),
            selected: 0,
            tab: Tab::Detail,
            sort: Sort::Status,
            context_limit,
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
        }

        self.sessions = sessions;
        self.sort_sessions();
        self.prune_transcript_cache();

        self.selected = anchor
            .and_then(|pid| self.sessions.iter().position(|s| s.pid == pid))
            .unwrap_or(self.selected)
            .min(self.sessions.len().saturating_sub(1));
    }

    fn sort_sessions(&mut self) {
        let limit = self.context_limit;
        let now = Self::now_ms();
        match self.sort {
            // Busy first, then the name, so the ordering is stable between refreshes.
            Sort::Status => self
                .sessions
                .sort_by(|a, b| match (a.status as u8).cmp(&(b.status as u8)) {
                    std::cmp::Ordering::Equal => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    other => other,
                }),
            Sort::Context => self.sessions.sort_by(|a, b| {
                b.context_ratio(limit)
                    .partial_cmp(&a.context_ratio(limit))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            Sort::Uptime => self
                .sessions
                .sort_by(|a, b| b.uptime_secs(now).cmp(&a.uptime_secs(now))),
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
