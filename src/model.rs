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

/// Context windows Claude Code actually ships. Assistant messages record the
/// base model id (`claude-opus-5`) even on the `[1m]` variant, so the window has
/// to come from `cost-state` lines, settings, observed usage, or the fallback
/// the user declared.
pub const CONTEXT_LIMIT_STANDARD: u64 = 200_000;
pub const CONTEXT_LIMIT_LONG: u64 = 1_000_000;

/// The suffix Claude Code puts on a long-context model id.
pub const LONG_CONTEXT_SUFFIX: &str = "[1m]";

/// Declaration order is the sort order: a session waiting on the user is the
/// one that needs attention, so it sorts above everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Busy,
    Idle,
    Other,
}

impl Status {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "waiting" => Status::Waiting,
            "busy" => Status::Busy,
            "idle" => Status::Idle,
            _ => Status::Other,
        }
    }

    /// Two cells wide for a waiting session, because a circle alone is too easy
    /// to miss in a column of dots; the question mark is what carries the state.
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Waiting => "●?",
            Status::Busy => "●",
            Status::Idle => "○",
            Status::Other => "·",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Waiting => "waiting",
            Status::Busy => "busy",
            Status::Idle => "idle",
            Status::Other => "?",
        }
    }

    /// The long form, for the detail pane where there is room to say what the
    /// session is waiting on.
    pub fn description(self) -> &'static str {
        match self {
            Status::Waiting => "waiting for input",
            other => other.label(),
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
    /// Model ids a `cost-state` line billed against the long window, with the
    /// `[1m]` suffix stripped so they compare against `model`.
    pub long_context_models: Vec<String>,
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

/// Where a session's context limit came from. Only `Assumed` is a guess, and it
/// is the only one the panels mark as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitSource {
    /// `--context-limit`, which outranks anything read from disk.
    Override,
    /// Usage already past the standard window, so the long one is a fact.
    Observed,
    /// A `cost-state` line billed this session's model at `[1m]`.
    Recorded,
    /// Settings name the long variant of the model this session runs.
    Configured,
    /// The user set a fallback window, by flag or in the config file.
    Declared,
    /// Nothing said otherwise.
    Assumed,
}

impl LimitSource {
    pub fn is_guess(self) -> bool {
        self == LimitSource::Assumed
    }
}

/// Does `configured` name the long variant of `model`? A user-wide `opus[1m]`
/// must not widen a session that switched to sonnet, so the family matters.
fn names_long_variant(configured: &str, model: &str) -> bool {
    let Some(base) = configured.strip_suffix(LONG_CONTEXT_SUFFIX) else {
        return false;
    };
    !base.is_empty() && model.to_lowercase().contains(&base.to_lowercase())
}

/// The two windows the user can set, as opposed to what a session's own
/// records show. `pinned` outranks every signal, including measured usage.
/// `fallback` replaces the built-in 200k for a session that says nothing about
/// its own window, which is every fresh long-context session: the model id on
/// an assistant message drops the `[1m]` suffix, and the `cost-state` line that
/// keeps it is only written at shutdown.
#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    pub pinned: Option<u64>,
    pub fallback: Option<u64>,
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
    /// The model settings name for this session's directory. Not proof of what
    /// the session runs now, but the only clue a young session gives.
    pub configured_model: Option<String>,
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

    /// The context limit for this session, and where the number came from.
    /// Strongest evidence first: measured usage cannot be argued with, a
    /// `cost-state` line is the session's own record, and settings are only the
    /// default it started from.
    pub fn context_limit(&self, limits: Limits) -> (u64, LimitSource) {
        if let Some(limit) = limits.pinned {
            return (limit, LimitSource::Override);
        }
        if self.detail.usage_peak > CONTEXT_LIMIT_STANDARD {
            return (CONTEXT_LIMIT_LONG, LimitSource::Observed);
        }
        if self.recorded_long() {
            return (CONTEXT_LIMIT_LONG, LimitSource::Recorded);
        }
        if self.configured_long() {
            return (CONTEXT_LIMIT_LONG, LimitSource::Configured);
        }
        if let Some(limit) = limits.fallback {
            return (limit, LimitSource::Declared);
        }
        (CONTEXT_LIMIT_STANDARD, LimitSource::Assumed)
    }

    /// Was this session's current model billed against the long window?
    fn recorded_long(&self) -> bool {
        let Some(model) = self.detail.model.as_deref() else {
            return false;
        };
        self.detail
            .long_context_models
            .iter()
            .any(|billed| billed == model)
    }

    fn configured_long(&self) -> bool {
        match (
            self.configured_model.as_deref(),
            self.detail.model.as_deref(),
        ) {
            (Some(configured), Some(model)) => names_long_variant(configured, model),
            _ => false,
        }
    }

    pub fn context_ratio(&self, limits: Limits) -> f64 {
        let used = self.detail.usage.as_ref().map(|u| u.total()).unwrap_or(0);
        let (limit, _) = self.context_limit(limits);
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

/// Read a token count the way the flags and the config file accept it: `1m`,
/// `200k`, `1.5m`, `750000`. The suffixes are the ones `tokens_short` prints,
/// so what the panels show is what the flags take.
pub fn tokens_parse(raw: &str) -> Option<u64> {
    let text = raw.trim().to_lowercase();
    let (digits, scale) = match text.strip_suffix('m') {
        Some(rest) => (rest, 1_000_000_f64),
        None => match text.strip_suffix('k') {
            Some(rest) => (rest, 1_000_f64),
            None => (text.as_str(), 1_f64),
        },
    };
    // A bare integer must not go through f64, which silently rounds past 2^53.
    if scale == 1.0 {
        return digits.parse::<u64>().ok().filter(|n| *n > 0);
    }
    let value: f64 = digits.parse().ok()?;
    let total = value * scale;
    if !(1.0..=u64::MAX as f64).contains(&total) {
        return None;
    }
    Some(total.round() as u64)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn session(peak: u64, used: u64) -> Session {
        Session {
            pid: 1,
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            name: "one".into(),
            status: Status::Busy,
            version: String::new(),
            kind: String::new(),
            entrypoint: String::new(),
            started_at_ms: 0,
            status_updated_at_ms: 0,
            proc: None,
            configured_model: None,
            detail: Detail {
                model: Some("claude-opus-5".into()),
                usage: Some(ContextUsage {
                    input: used,
                    ..Default::default()
                }),
                usage_peak: peak,
                ..Default::default()
            },
        }
    }

    #[test]
    fn usage_past_the_standard_window_settles_the_limit() {
        let (limit, source) = session(386_849, 386_849).context_limit(Limits::default());
        assert_eq!(limit, CONTEXT_LIMIT_LONG);
        assert_eq!(source, LimitSource::Observed);
        assert!(!source.is_guess());
    }

    #[test]
    fn nothing_known_falls_back_to_the_standard_window() {
        let (limit, source) = session(169_456, 169_456).context_limit(Limits::default());
        assert_eq!(limit, CONTEXT_LIMIT_STANDARD);
        assert_eq!(source, LimitSource::Assumed);
        assert!(source.is_guess());
    }

    /// The regression: 169k of a 1M window read as 85% instead of 17%.
    #[test]
    fn a_recorded_long_model_widens_a_session_under_200k() {
        let mut session = session(169_456, 169_456);
        session.detail.long_context_models = vec!["claude-opus-5".into()];

        let (limit, source) = session.context_limit(Limits::default());
        assert_eq!(limit, CONTEXT_LIMIT_LONG);
        assert_eq!(source, LimitSource::Recorded);
        assert!((session.context_ratio(Limits::default()) - 0.169456).abs() < 1e-9);
    }

    #[test]
    fn a_recorded_model_the_session_left_is_ignored() {
        let mut session = session(169_456, 169_456);
        session.detail.long_context_models = vec!["claude-fable-5".into()];
        assert_eq!(
            session.context_limit(Limits::default()).1,
            LimitSource::Assumed
        );
    }

    #[test]
    fn settings_widen_only_the_family_they_name() {
        let mut session = session(100, 100);
        session.configured_model = Some("opus[1m]".into());
        assert_eq!(
            session.context_limit(Limits::default()).1,
            LimitSource::Configured
        );

        session.detail.model = Some("claude-sonnet-5".into());
        assert_eq!(
            session.context_limit(Limits::default()).1,
            LimitSource::Assumed
        );

        session.detail.model = Some("claude-opus-5".into());
        session.configured_model = Some("opus".into());
        assert_eq!(
            session.context_limit(Limits::default()).1,
            LimitSource::Assumed
        );
    }

    /// What the flag and the config file are for: a fresh long-context session
    /// records nothing that names its window, so 169k read as 85% of 200k.
    #[test]
    fn a_declared_fallback_replaces_the_built_in_window() {
        let session = session(169_456, 169_456);
        let limits = Limits {
            fallback: Some(CONTEXT_LIMIT_LONG),
            ..Default::default()
        };

        let (limit, source) = session.context_limit(limits);
        assert_eq!(limit, CONTEXT_LIMIT_LONG);
        assert_eq!(source, LimitSource::Declared);
        assert!(!source.is_guess());
        assert!((session.context_ratio(limits) - 0.169456).abs() < 1e-9);
    }

    /// The fallback is the weakest signal, so a session that proves a wider
    /// window keeps it. A narrow fallback must not shrink the gauge back.
    #[test]
    fn a_declared_fallback_yields_to_the_session_own_record() {
        let mut session = session(386_849, 386_849);
        let limits = Limits {
            fallback: Some(CONTEXT_LIMIT_STANDARD),
            ..Default::default()
        };
        assert_eq!(session.context_limit(limits).1, LimitSource::Observed);

        session.detail.usage_peak = 100;
        session.detail.long_context_models = vec!["claude-opus-5".into()];
        assert_eq!(session.context_limit(limits).1, LimitSource::Recorded);
    }

    #[test]
    fn token_counts_read_the_way_the_panels_print_them() {
        assert_eq!(tokens_parse("1m"), Some(1_000_000));
        assert_eq!(tokens_parse("1M"), Some(1_000_000));
        assert_eq!(tokens_parse("200k"), Some(200_000));
        assert_eq!(tokens_parse(" 1.5m "), Some(1_500_000));
        assert_eq!(tokens_parse("750000"), Some(750_000));
        // Every one of these would otherwise pin the window at zero or panic.
        for bad in ["0", "0m", "m", "", "1mb", "-5", "1e400m", "abc", "1 m"] {
            assert_eq!(tokens_parse(bad), None, "input: {bad:?}");
        }
    }

    #[test]
    fn an_explicit_limit_outranks_every_signal() {
        let mut session = session(386_849, 386_849);
        session.configured_model = Some("opus[1m]".into());

        let limits = Limits {
            pinned: Some(500_000),
            fallback: Some(CONTEXT_LIMIT_LONG),
        };
        let (limit, source) = session.context_limit(limits);
        assert_eq!(limit, 500_000);
        assert_eq!(source, LimitSource::Override);
        assert!(!source.is_guess());
    }
}
