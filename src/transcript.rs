//! Reads a session's JSONL transcript.
//!
//! The file is append-only and grows without bound, so only its tail is read.
//! Alongside the messages, Claude Code writes single-purpose metadata lines
//! (`ai-title`, `permission-mode`, `worktree-state`, `pr-link`, ...) and those
//! carry most of what the detail pane shows.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::model::{
    ContextUsage, Detail, PullRequest, Worktree, MODEL_SYNTHETIC, PROMPT_CHARS_MAX,
    PROMPT_SEARCH_BYTES_MAX,
    PROMPT_SEARCH_CHUNK_BYTES, TRANSCRIPT_LINES_MAX, TRANSCRIPT_TAIL_BYTES_MAX,
};

/// Find `projects/*/<session_id>.jsonl`.
///
/// The project directory is the session's cwd with its separators and dots
/// rewritten, but that encoding is undocumented, so the directories are searched
/// instead of reconstructed. Callers cache the result: a session's transcript
/// path does not change while it runs.
pub fn locate(claude_home: &Path, session_id: &str) -> Option<PathBuf> {
    let projects = claude_home.join("projects");
    let entries = std::fs::read_dir(&projects).ok()?;
    let filename = format!("{session_id}.jsonl");
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let candidate = entry.path().join(&filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn read(path: &Path) -> Detail {
    let mut detail = Detail {
        transcript: Some(path.to_path_buf()),
        ..Default::default()
    };

    // The transcript is rewritten on every turn, so its mtime is the session's
    // real last-activity time — cheaper and more reliable than parsing the
    // timestamp strings inside.
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(modified) = meta.modified() {
            detail.transcript_age_secs = SystemTime::now()
                .duration_since(modified)
                .ok()
                .map(|d| d.as_secs());
        }
    }

    let lines = match read_tail_lines(path) {
        Ok(lines) => lines,
        Err(err) => {
            detail.read_error = Some(err.to_string());
            return detail;
        }
    };

    parse_lines(&lines, &mut detail);

    // A long turn can push the human's prompt out of the tail window, leaving it
    // full of tool results. Search further back rather than reporting none.
    if detail.last_prompt.is_none() {
        detail.last_prompt = find_prompt_before_tail(path);
    }

    detail
}

/// Scan backwards from where the tail began, one chunk at a time, and stop at the
/// first prompt found. Reads at most `PROMPT_SEARCH_BYTES_MAX` beyond the tail.
fn find_prompt_before_tail(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();

    let mut end = len.saturating_sub(TRANSCRIPT_TAIL_BYTES_MAX);
    let floor = end.saturating_sub(PROMPT_SEARCH_BYTES_MAX);

    while end > floor {
        let start = end.saturating_sub(PROMPT_SEARCH_CHUNK_BYTES).max(floor);
        let span = (end - start) as usize;

        file.seek(SeekFrom::Start(start)).ok()?;
        let mut buffer = vec![0u8; span];
        file.read_exact(&mut buffer).ok()?;
        let text = String::from_utf8_lossy(&buffer);

        // Reversed, because the newest prompt in the chunk is the one wanted.
        // The first line is dropped whenever the chunk starts mid-line.
        let mut lines: Vec<&str> = text.lines().collect();
        if start > 0 && !lines.is_empty() {
            lines.remove(0);
        }
        for line in lines.iter().rev() {
            let Ok(entry) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if entry.get("type").and_then(Value::as_str) != Some("user") {
                continue;
            }
            if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            if let Some(text) = user_prompt_text(&entry) {
                return Some(text);
            }
        }

        end = start;
    }
    None
}

/// Read at most the last `TRANSCRIPT_TAIL_BYTES_MAX` and split into lines,
/// dropping the leading fragment when the window lands mid-line.
fn read_tail_lines(path: &Path) -> std::io::Result<Vec<String>> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TRANSCRIPT_TAIL_BYTES_MAX);
    file.seek(SeekFrom::Start(start))?;

    let mut buffer = Vec::with_capacity(TRANSCRIPT_TAIL_BYTES_MAX as usize);
    file.take(TRANSCRIPT_TAIL_BYTES_MAX).read_to_end(&mut buffer)?;
    let text = String::from_utf8_lossy(&buffer);

    let mut lines: Vec<String> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if index == 0 && start > 0 {
            continue;
        }
        if line.is_empty() {
            continue;
        }
        lines.push(line.to_string());
    }
    if lines.len() > TRANSCRIPT_LINES_MAX {
        lines.drain(..lines.len() - TRANSCRIPT_LINES_MAX);
    }
    Ok(lines)
}

/// State that only makes sense once every line is read: the prompt a
/// `last-prompt` line points at, and which of the three title kinds won.
#[derive(Default)]
struct Pending {
    prompt_leaf: Option<String>,
    prompts_by_uuid: HashMap<String, String>,
    newest_prompt: Option<String>,
    ai_title: Option<String>,
    custom_title: Option<String>,
    agent_name: Option<String>,
}

impl Pending {
    fn apply(self, detail: &mut Detail) {
        // A title the user set outranks one the model generated.
        detail.title = self.custom_title.or(self.agent_name).or(self.ai_title);
        detail.last_prompt = self
            .prompt_leaf
            .and_then(|uuid| self.prompts_by_uuid.get(&uuid).cloned())
            .or(self.newest_prompt);
    }
}

fn parse_lines(lines: &[String], detail: &mut Detail) {
    let mut pending = Pending::default();

    for line in lines {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        // Subagent traffic is reported separately; it must not overwrite the
        // parent session's model, usage or prompt.
        if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }

        if let Some(branch) = entry.get("gitBranch").and_then(Value::as_str) {
            if !branch.is_empty() {
                detail.git_branch = Some(branch.to_string());
            }
        }

        parse_entry(&entry, detail, &mut pending);
    }

    pending.apply(detail);
}

fn parse_entry(entry: &Value, detail: &mut Detail, pending: &mut Pending) {
    let field = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);

    match entry.get("type").and_then(Value::as_str).unwrap_or("") {
        "assistant" => parse_assistant(entry, detail),
        "user" => {
            // The prompt is named by a `last-prompt` line rather than being the
            // newest user entry, because tool results are user entries too.
            if let Some(text) = user_prompt_text(entry) {
                if let Some(uuid) = entry.get("uuid").and_then(Value::as_str) {
                    pending.prompts_by_uuid.insert(uuid.to_string(), text.clone());
                }
                pending.newest_prompt = Some(text);
            }
            detail.totals.user_messages += 1;
        }
        "last-prompt" => pending.prompt_leaf = field("leafUuid"),
        "ai-title" => pending.ai_title = field("aiTitle"),
        "custom-title" => pending.custom_title = field("customTitle"),
        "agent-name" => pending.agent_name = field("agentName"),
        "permission-mode" => detail.permission_mode = field("permissionMode"),
        "mode" => detail.mode = field("mode"),
        "worktree-state" => parse_worktree(entry, detail),
        "pr-link" => parse_pull_request(entry, detail),
        _ => {}
    }
}

fn parse_assistant(entry: &Value, detail: &mut Detail) {
    let Some(message) = entry.get("message") else {
        return;
    };

    // A locally-generated message reports zero tokens against no model. Counting
    // it would blank the context gauge and mislabel the model.
    if message.get("model").and_then(Value::as_str) == Some(MODEL_SYNTHETIC) {
        return;
    }

    detail.totals.assistant_messages += 1;
    if let Some(effort) = entry.get("effort").and_then(Value::as_str) {
        detail.effort = Some(effort.to_string());
    }
    if let Some(model) = message.get("model").and_then(Value::as_str) {
        detail.model = Some(model.to_string());
    }
    let Some(usage) = message.get("usage") else {
        return;
    };

    let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let current = ContextUsage {
        input: field("input_tokens"),
        cache_read: field("cache_read_input_tokens"),
        cache_creation: field("cache_creation_input_tokens"),
        output: field("output_tokens"),
    };

    detail.totals.output_tokens += current.output;
    detail.totals.cache_creation_tokens += current.cache_creation;
    detail.totals.thinking_tokens += usage
        .get("output_tokens_details")
        .and_then(|d| d.get("thinking_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    detail.totals.web_searches += usage
        .get("server_tool_use")
        .and_then(|s| s.get("web_search_requests"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if let Some(tier) = usage.get("service_tier").and_then(Value::as_str) {
        detail.service_tier = Some(tier.to_string());
    }

    detail.usage_peak = detail.usage_peak.max(current.total());
    detail.usage = Some(current);
}

fn parse_worktree(entry: &Value, detail: &mut Detail) {
    // Leaving a worktree is recorded as an explicit null, so a null has to clear
    // the worktree the session was in, not be skipped as missing data.
    let state = match entry.get("worktreeSession") {
        Some(Value::Object(_)) => entry.get("worktreeSession").unwrap(),
        _ => {
            detail.worktree = None;
            return;
        }
    };
    let get = |key: &str| {
        state
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    detail.worktree = Some(Worktree {
        name: get("worktreeName"),
        branch: get("worktreeBranch"),
        original_branch: get("originalBranch"),
        path: get("worktreePath"),
    });
}

fn parse_pull_request(entry: &Value, detail: &mut Detail) {
    let Some(number) = entry.get("prNumber").and_then(Value::as_u64) else {
        return;
    };
    detail.pull_request = Some(PullRequest {
        number,
        url: entry
            .get("prUrl")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        repository: entry
            .get("prRepository")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    });
}

/// Pull the human's own words out of a `user` entry.
///
/// Returns None for the entries that are mechanically user-role but were not
/// typed by anyone: tool results, injected reminders, and resumption caveats.
fn user_prompt_text(entry: &Value) -> Option<String> {
    if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let content = entry.get("message")?.get("content")?;

    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            let mut collected = String::new();
            for block in blocks {
                // Any tool_result block makes this entry tool output, not a prompt.
                if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                    return None;
                }
                if let Some(part) = block.get("text").and_then(Value::as_str) {
                    if !collected.is_empty() {
                        collected.push('\n');
                    }
                    collected.push_str(part);
                }
            }
            collected
        }
        _ => return None,
    };

    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("<system-reminder>")
        || trimmed.starts_with("<command-name>")
        || trimmed.starts_with("<local-command")
        || trimmed.starts_with("Caveat:")
    {
        return None;
    }

    let mut out: String = trimmed.chars().take(PROMPT_CHARS_MAX).collect();
    if trimmed.chars().count() > PROMPT_CHARS_MAX {
        out.push('…');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transcript shaped like the real thing: metadata lines, a tool result
    /// that must not be mistaken for a prompt, and a subagent line that must not
    /// overwrite the parent's state.
    fn fixture() -> Vec<String> {
        [
            r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
            r#"{"type":"permission-mode","permissionMode":"auto","sessionId":"s"}"#,
            r#"{"type":"ai-title","aiTitle":"generated","sessionId":"s"}"#,
            r#"{"type":"agent-name","agentName":"named","sessionId":"s"}"#,
            r#"{"type":"custom-title","customTitle":"chosen","sessionId":"s"}"#,
            r#"{"type":"user","uuid":"u1","gitBranch":"main","message":{"role":"user","content":"the real prompt"}}"#,
            r#"{"type":"user","uuid":"u2","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"user","uuid":"u3","message":{"role":"user","content":"<system-reminder>ignore me</system-reminder>"}}"#,
            r#"{"type":"assistant","effort":"high","message":{"model":"claude-opus-5","usage":{"input_tokens":5,"cache_read_input_tokens":90000,"cache_creation_input_tokens":100,"output_tokens":400,"service_tier":"standard","output_tokens_details":{"thinking_tokens":50},"server_tool_use":{"web_search_requests":1}}}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku-4-5","usage":{"input_tokens":1,"cache_read_input_tokens":999999,"output_tokens":1}}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_read_input_tokens":50000,"cache_creation_input_tokens":10,"output_tokens":20}}}"#,
            r#"{"type":"pr-link","prNumber":863,"prUrl":"https://example.test/pull/863","prRepository":"example/repo"}"#,
            r#"{"type":"last-prompt","leafUuid":"u1","sessionId":"s"}"#,
        ]
        .iter()
        .map(|line| line.to_string())
        .collect()
    }

    fn parsed() -> Detail {
        let mut detail = Detail::default();
        parse_lines(&fixture(), &mut detail);
        detail
    }

    #[test]
    fn context_comes_from_the_newest_message_and_peak_is_kept() {
        let detail = parsed();
        let usage = detail.usage.expect("usage");
        // The last non-sidechain assistant message, not the largest one.
        assert_eq!(usage.total(), 2 + 50_000 + 10 + 20);
        // The peak survives the drop, which is what compaction looks like.
        assert_eq!(detail.usage_peak, 5 + 90_000 + 100 + 400);
    }

    #[test]
    fn a_subagent_message_does_not_overwrite_the_session() {
        let detail = parsed();
        assert_eq!(detail.model.as_deref(), Some("claude-opus-5"));
        assert!(detail.usage_peak < 999_999);
        // Only the two parent assistant messages are counted.
        assert_eq!(detail.totals.assistant_messages, 2);
    }

    #[test]
    fn a_synthetic_message_does_not_blank_the_context() {
        let mut lines = fixture();
        // Claude Code appends this after an interrupt: zero tokens, no model.
        lines.push(
            r#"{"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"No response requested."}],"usage":{"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}}}"#
                .to_string(),
        );

        let mut detail = Detail::default();
        parse_lines(&lines, &mut detail);

        assert_eq!(detail.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(detail.usage.expect("usage").total(), 2 + 50_000 + 10 + 20);
        assert_eq!(detail.totals.assistant_messages, 2);
    }

    #[test]
    fn a_title_the_user_set_outranks_a_generated_one() {
        assert_eq!(parsed().title.as_deref(), Some("chosen"));
    }

    #[test]
    fn the_prompt_is_the_one_last_prompt_names() {
        let detail = parsed();
        assert_eq!(detail.last_prompt.as_deref(), Some("the real prompt"));
    }

    #[test]
    fn tool_results_and_injected_text_are_not_prompts() {
        let tool_result = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "tool_result", "content": "ok"}]}
        });
        assert_eq!(user_prompt_text(&tool_result), None);

        let reminder = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": "<system-reminder>hi</system-reminder>"}
        });
        assert_eq!(user_prompt_text(&reminder), None);

        let meta = serde_json::json!({
            "type": "user", "isMeta": true,
            "message": {"role": "user", "content": "bookkeeping"}
        });
        assert_eq!(user_prompt_text(&meta), None);
    }

    #[test]
    fn metadata_lines_populate_the_detail_pane() {
        let detail = parsed();
        assert_eq!(detail.permission_mode.as_deref(), Some("auto"));
        assert_eq!(detail.mode.as_deref(), Some("normal"));
        assert_eq!(detail.git_branch.as_deref(), Some("main"));
        assert_eq!(detail.effort.as_deref(), Some("high"));
        assert_eq!(detail.service_tier.as_deref(), Some("standard"));
        let pr = detail.pull_request.expect("pr");
        assert_eq!(pr.number, 863);
        assert_eq!(pr.repository, "example/repo");
        assert_eq!(detail.totals.thinking_tokens, 50);
        assert_eq!(detail.totals.web_searches, 1);
    }

    #[test]
    fn leaving_a_worktree_clears_it() {
        let lines: Vec<String> = [
            r#"{"type":"worktree-state","worktreeSession":{"worktreeName":"fix/thing","worktreeBranch":"worktree-fix+thing","originalBranch":"main","worktreePath":"/tmp/wt"}}"#,
            r#"{"type":"worktree-state","worktreeSession":null}"#,
        ]
        .iter()
        .map(|l| l.to_string())
        .collect();

        let mut detail = Detail::default();
        parse_lines(&lines, &mut detail);
        assert!(
            detail.worktree.is_none(),
            "a null worktreeSession must clear the worktree, not leave a blank one"
        );
    }

    #[test]
    fn entering_a_worktree_records_it() {
        let lines = vec![
            r#"{"type":"worktree-state","worktreeSession":{"worktreeName":"fix/thing","worktreeBranch":"worktree-fix+thing","originalBranch":"main","worktreePath":"/tmp/wt"}}"#
                .to_string(),
        ];
        let mut detail = Detail::default();
        parse_lines(&lines, &mut detail);
        let worktree = detail.worktree.expect("worktree");
        assert_eq!(worktree.name, "fix/thing");
        assert_eq!(worktree.original_branch, "main");
    }

    #[test]
    fn an_oversized_prompt_is_truncated_rather_than_held_whole() {
        let long = "x".repeat(PROMPT_CHARS_MAX * 2);
        let entry = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": long}
        });
        let text = user_prompt_text(&entry).expect("text");
        assert_eq!(text.chars().count(), PROMPT_CHARS_MAX + 1);
        assert!(text.ends_with('…'));
    }
}
