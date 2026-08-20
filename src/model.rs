//! Types shared across the app, and every fixed limit in one place.

use std::path::PathBuf;

/// Upper bounds. Each read path is capped so a runaway file or a directory full
/// of stale entries cannot stall a redraw.
pub const SESSIONS_MAX: usize = 256;
pub const SUBAGENTS_MAX: usize = 64;
pub const TRANSCRIPT_TAIL_BYTES_MAX: u64 = 1024 * 1024;
pub const TRANSCRIPT_LINES_MAX: usize = 4096;
pub const PROMPT_CHARS_MAX: usize = 4000;
/// How far back to look for the last human prompt once the tail has been read,
/// and the chunk size that search steps in.
pub const PROMPT_SEARCH_BYTES_MAX: u64 = 16 * 1024 * 1024;
pub const PROMPT_SEARCH_CHUNK_BYTES: u64 = 1024 * 1024;

/// The model id Claude Code writes on assistant messages it generated locally,
/// such as "No response requested." after an interrupt. They carry all-zero
/// usage, so they must not be read as the session's real state.
pub const MODEL_SYNTHETIC: &str = "<synthetic>";

/// Context windows Claude Code actually ships. The transcript records the base
/// model id (`claude-opus-5`) even when the session runs the `[1m]` variant, so
/// the limit cannot be read directly and is inferred from observed usage.
pub const CONTEXT_LIMIT_STANDARD: u64 = 200_000;
pub const CONTEXT_LIMIT_LONG: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Busy,
    Idle,
    Other,
}

impl Status {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "busy" => Status::Busy,
            "idle" => Status::Idle,
            _ => Status::Other,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Status::Busy => "●",
            Status::Idle => "○",
            Status::Other => "·",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Busy => "busy",
            Status::Idle => "idle",
            Status::Other => "?",
        }
    }
}

/// One line of `ps` output for a live session process.
#[derive(Debug, Clone)]
pub struct ProcStat {
    pub rss_kib: u64,
    pub cpu_percent: f64,
}

/// Token counts from the most recent assistant message.
#[derive(Debug, Clone, Default)]
pub struct ContextUsage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub output: u64,
}

impl ContextUsage {
    /// What the next request will carry: everything already in the window plus
    /// the reply just written into it.
    pub fn total(&self) -> u64 {
        self.input + self.cache_read + self.cache_creation + self.output
    }
}

/// Totals accumulated over the parsed tail, not the whole session. The tail is
/// capped, so these describe recent activity rather than a lifetime bill.
#[derive(Debug, Clone, Default)]
pub struct TailTotals {
    pub assistant_messages: usize,
    pub user_messages: usize,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub cache_creation_tokens: u64,
    pub web_searches: u64,
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub name: String,
    pub branch: String,
    pub original_branch: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub repository: String,
}

#[derive(Debug, Clone)]
pub struct Subagent {
    pub name: String,
    pub agent_type: String,
    pub description: String,
    pub spawn_depth: u64,
    pub age_secs: Option<u64>,
    pub bytes: u64,
}

/// Everything recovered from a session's transcript file.
#[derive(Debug, Clone, Default)]
pub struct Detail {
    pub transcript: Option<PathBuf>,
    pub transcript_age_secs: Option<u64>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub service_tier: Option<String>,
    pub usage: Option<ContextUsage>,
    /// Highest total seen in the tail. Drives the context-limit inference, and
    /// survives the dip that compaction puts in the latest message.
    pub usage_peak: u64,
    pub totals: TailTotals,
    pub title: Option<String>,
    pub git_branch: Option<String>,
    pub permission_mode: Option<String>,
    pub mode: Option<String>,
    pub worktree: Option<Worktree>,
    pub pull_request: Option<PullRequest>,
    pub last_prompt: Option<String>,
    pub subagents: Vec<Subagent>,
    /// Set when the transcript exists but could not be read or parsed.
    pub read_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub pid: u32,
    pub session_id: String,
    pub cwd: PathBuf,
    pub name: String,
    pub status: Status,
    pub version: String,
    pub kind: String,
    pub entrypoint: String,
    pub started_at_ms: i64,
    pub status_updated_at_ms: i64,
    pub proc: Option<ProcStat>,
    pub detail: Detail,
}

impl Session {
    /// Directory name only. Two sessions often share a repo, so the list shows
    /// this while the detail pane shows the full path.
    pub fn dir_label(&self) -> String {
        self.cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.cwd.to_string_lossy().to_string())
    }

    /// How long the session has held its current status. This is what separates
    /// a session working on something from one wedged in `busy`.
    pub fn status_age_secs(&self, now_ms: i64) -> Option<u64> {
        if self.status_updated_at_ms <= 0 {
            return None;
        }
        let delta = now_ms - self.status_updated_at_ms;
        if delta > 0 {
            Some((delta / 1000) as u64)
        } else {
            Some(0)
        }
    }

    pub fn uptime_secs(&self, now_ms: i64) -> u64 {
        let delta = now_ms - self.started_at_ms;
        if delta > 0 {
            (delta / 1000) as u64
        } else {
            0
        }
    }

    /// The context limit for this session, and whether it had to be inferred.
    pub fn context_limit(&self, override_limit: Option<u64>) -> (u64, bool) {
        if let Some(limit) = override_limit {
            return (limit, false);
        }
        if self.detail.usage_peak > CONTEXT_LIMIT_STANDARD {
            (CONTEXT_LIMIT_LONG, true)
        } else {
            (CONTEXT_LIMIT_STANDARD, true)
        }
    }

    pub fn context_ratio(&self, override_limit: Option<u64>) -> f64 {
        let used = self.detail.usage.as_ref().map(|u| u.total()).unwrap_or(0);
        let (limit, _) = self.context_limit(override_limit);
        if limit == 0 {
            return 0.0;
        }
        (used as f64 / limit as f64).clamp(0.0, 1.0)
    }
}

/// Format a token count the way the panels want it: `82k`, `1.2M`, `940`.
pub fn tokens_short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

/// Compact duration: `4d2h`, `3h12m`, `18m54s`, `41s`.
pub fn duration_short(secs: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    if secs >= DAY {
        format!("{}d{}h", secs / DAY, (secs % DAY) / HOUR)
    } else if secs >= HOUR {
        format!("{}h{}m", secs / HOUR, (secs % HOUR) / MINUTE)
    } else if secs >= MINUTE {
        format!("{}m{}s", secs / MINUTE, secs % MINUTE)
    } else {
        format!("{}s", secs)
    }
}
