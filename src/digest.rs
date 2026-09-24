//! Writes a readable digest of a session log.
//!
//! The digest keeps prompts, thinking, and tool calls, grouped by agent.

use std::fmt::Write as _;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::{ClientKind, Session};

/// One thing an agent did.
#[derive(Debug, PartialEq)]
enum Step {
    Prompt(String),
    Thinking(String),
    Call {
        tool: String,
        summary: String,
        input: String,
    },
}

/// The steps of one agent, oldest first.
struct Track {
    title: String,
    steps: Vec<Step>,
}

/// Input keys that name a call's target.
const INPUT_KEYS: [&str; 9] = [
    "command",
    "cmd",
    "file_path",
    "path",
    "pattern",
    "url",
    "query",
    "skill",
    "prompt",
];

/// Write the digest and return its path.
pub fn write(session: &Session) -> std::io::Result<PathBuf> {
    let Some(log) = session.detail.transcript.as_deref() else {
        return Err(std::io::Error::other("no log for this session"));
    };
    let mut tracks = vec![Track {
        title: "main".to_string(),
        steps: read_steps(session.client, log)?,
    }];
    if session.client == ClientKind::Claude {
        tracks.extend(subagent_tracks(log));
    }

    let dir = std::env::temp_dir().join("dancefloor");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "{}-{}.md",
        session.client.label().to_lowercase(),
        session.session_id
    ));
    std::fs::write(&path, render(session, log, &tracks))?;
    Ok(path)
}

fn read_steps(client: ClientKind, log: &Path) -> std::io::Result<Vec<Step>> {
    let file = std::fs::File::open(log)?;
    let mut steps = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let Ok(entry) = serde_json::from_str::<Value>(&line?) else {
            continue;
        };
        match client {
            ClientKind::Claude => claude_steps(&entry, &mut steps),
            ClientKind::Codex => codex_steps(&entry, &mut steps),
            ClientKind::Pi => pi_steps(&entry, &mut steps),
        }
    }
    Ok(steps)
}

fn claude_steps(entry: &Value, steps: &mut Vec<Step>) {
    // Old logs inline subagents; skip them.
    if entry.get("isSidechain").and_then(Value::as_bool) == Some(true)
        && entry.get("agentId").is_none()
    {
        return;
    }
    let content = entry.pointer("/message/content");
    match entry.get("type").and_then(Value::as_str) {
        Some("user") => {
            if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
                return;
            }
            if let Some(text) = content.and_then(user_text) {
                steps.push(Step::Prompt(text));
            }
        }
        Some("assistant") => {
            for block in content.and_then(Value::as_array).into_iter().flatten() {
                match block.get("type").and_then(Value::as_str) {
                    Some("thinking") => push_thinking(steps, block.get("thinking")),
                    Some("tool_use") => push_call(steps, block.get("name"), block.get("input")),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn codex_steps(entry: &Value, steps: &mut Vec<Step>) {
    let Some(payload) = entry.get("payload") else {
        return;
    };
    match (
        entry.get("type").and_then(Value::as_str),
        payload.get("type").and_then(Value::as_str),
    ) {
        (Some("event_msg"), Some("user_message")) => {
            if let Some(text) = payload.get("message").and_then(Value::as_str) {
                steps.push(Step::Prompt(text.trim().to_string()));
            }
        }
        (Some("response_item"), Some("reasoning")) => {
            let parts = ["summary", "content"]
                .iter()
                .filter_map(|key| payload.get(*key).and_then(Value::as_array))
                .flatten()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n");
            if parts.trim().is_empty() {
                steps.push(Step::Thinking("(encrypted)".to_string()));
            } else {
                steps.push(Step::Thinking(parts.trim().to_string()));
            }
        }
        (Some("response_item"), Some("function_call")) => {
            // Arguments arrive as a JSON string.
            let input = payload
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
            push_call(steps, payload.get("name"), input.as_ref());
        }
        (Some("response_item"), Some("custom_tool_call")) => {
            push_call(steps, payload.get("name"), payload.get("input"));
        }
        (Some("response_item"), Some("local_shell_call")) => {
            push_call(
                steps,
                Some(&Value::from("shell")),
                payload.pointer("/action/command"),
            );
        }
        _ => {}
    }
}

fn pi_steps(entry: &Value, steps: &mut Vec<Step>) {
    if entry.get("type").and_then(Value::as_str) != Some("message") {
        return;
    }
    let content = entry.pointer("/message/content");
    match entry.pointer("/message/role").and_then(Value::as_str) {
        Some("user") => {
            if let Some(text) = content.and_then(user_text) {
                steps.push(Step::Prompt(text));
            }
        }
        Some("assistant") => {
            for block in content.and_then(Value::as_array).into_iter().flatten() {
                match block.get("type").and_then(Value::as_str) {
                    Some("thinking") => push_thinking(steps, block.get("thinking")),
                    Some("toolCall") => push_call(steps, block.get("name"), block.get("arguments")),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Plain prompt text; tool results return None.
fn user_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn push_thinking(steps: &mut Vec<Step>, text: Option<&Value>) {
    match text.and_then(Value::as_str).map(str::trim) {
        Some(text) if !text.is_empty() => steps.push(Step::Thinking(text.to_string())),
        _ => steps.push(Step::Thinking("(redacted)".to_string())),
    }
}

fn push_call(steps: &mut Vec<Step>, name: Option<&Value>, input: Option<&Value>) {
    let Some(tool) = name.and_then(Value::as_str) else {
        return;
    };
    let summary = input
        .and_then(|i| i.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    steps.push(Step::Call {
        tool: tool.to_string(),
        summary,
        input: input.map(call_input).unwrap_or_default(),
    });
}

/// The input that says what the call did.
fn call_input(input: &Value) -> String {
    if let Some(text) = input.as_str() {
        return text.to_string();
    }
    if let Some(parts) = input.as_array() {
        return parts
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" ");
    }
    for key in INPUT_KEYS {
        match input.get(key) {
            Some(Value::String(text)) => return text.clone(),
            Some(Value::Array(parts)) => return call_input(&Value::Array(parts.clone())),
            _ => {}
        }
    }
    serde_json::to_string_pretty(input).unwrap_or_default()
}

/// One track per file in `subagents/`.
fn subagent_tracks(log: &Path) -> Vec<Track> {
    let (Some(parent), Some(stem)) = (log.parent(), log.file_stem()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(parent.join(stem).join("subagents")) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, Track)> = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(steps) = read_steps(ClientKind::Claude, &path) else {
            continue;
        };
        let started = std::fs::metadata(&path)
            .and_then(|m| m.created().or_else(|_| m.modified()))
            .unwrap_or(std::time::UNIX_EPOCH);
        found.push((
            started,
            Track {
                title: agent_title(&path),
                steps,
            },
        ));
    }
    found.sort_by_key(|(started, _)| *started);
    found.into_iter().map(|(_, track)| track).collect()
}

/// `<type>: <description>` from the meta file.
fn agent_title(conversation: &Path) -> String {
    let id = conversation
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("agent")
        .to_string();
    let meta = conversation.with_extension("meta.json");
    let Some(meta) = std::fs::read_to_string(meta)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return id;
    };
    let field = |key: &str| {
        meta.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match (field("agentType"), field("description")) {
        (kind, text) if kind.is_empty() && text.is_empty() => id,
        (kind, text) => format!("{kind}: {text} ({id})"),
    }
}

fn render(session: &Session, log: &Path, tracks: &[Track]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {} · {}", session.name, session.client.label());
    let _ = writeln!(out);
    let _ = writeln!(out, "- log: `{}`", log.display());
    let _ = writeln!(out, "- cwd: `{}`", session.cwd.display());
    let _ = writeln!(out, "- agents: {}", tracks.len());
    for track in tracks {
        let _ = writeln!(out);
        let _ = writeln!(out, "## Agent {}", track.title);
        if track.steps.is_empty() {
            let _ = writeln!(out, "\n_No steps recorded._");
        }
        for step in &track.steps {
            let _ = writeln!(out);
            match step {
                Step::Prompt(text) => {
                    let _ = writeln!(out, "### Prompt\n");
                    let _ = writeln!(out, "{}", quote(text));
                }
                Step::Thinking(text) => {
                    let _ = writeln!(out, "**Thinking**\n");
                    let _ = writeln!(out, "{}", quote(text));
                }
                Step::Call {
                    tool,
                    summary,
                    input,
                } => {
                    if summary.is_empty() {
                        let _ = writeln!(out, "**{tool}**\n");
                    } else {
                        let _ = writeln!(out, "**{tool}**: {summary}\n");
                    }
                    let _ = writeln!(out, "````\n{}\n````", input.trim_end());
                }
            }
        }
    }
    out
}

fn quote(text: &str) -> String {
    text.lines()
        .map(|line| format!("> {line}").trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(client: ClientKind, lines: &[&str]) -> Vec<Step> {
        let mut steps = Vec::new();
        for line in lines {
            let entry: Value = serde_json::from_str(line).unwrap();
            match client {
                ClientKind::Claude => claude_steps(&entry, &mut steps),
                ClientKind::Codex => codex_steps(&entry, &mut steps),
                ClientKind::Pi => pi_steps(&entry, &mut steps),
            }
        }
        steps
    }

    #[test]
    fn claude_keeps_prompt_thinking_and_calls() {
        let got = steps(
            ClientKind::Claude,
            &[
                r#"{"type":"user","message":{"content":"fix it"}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"look first"},{"type":"text","text":"hi"},{"type":"tool_use","name":"Bash","input":{"command":"ls\n-la","description":"List"}}]}}"#,
            ],
        );
        assert_eq!(
            got,
            vec![
                Step::Prompt("fix it".into()),
                Step::Thinking("look first".into()),
                Step::Call {
                    tool: "Bash".into(),
                    summary: "List".into(),
                    input: "ls\n-la".into(),
                },
            ]
        );
    }

    #[test]
    fn claude_skips_inline_sidechains() {
        let got = steps(
            ClientKind::Claude,
            &[r#"{"type":"user","isSidechain":true,"message":{"content":"sub"}}"#],
        );
        assert!(got.is_empty());
    }

    #[test]
    fn codex_reads_arguments_and_encrypted_reasoning() {
        let got = steps(
            ClientKind::Codex,
            &[
                r#"{"type":"response_item","payload":{"type":"reasoning","summary":[]}}"#,
                r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"git status\"}"}}"#,
            ],
        );
        assert_eq!(
            got,
            vec![
                Step::Thinking("(encrypted)".into()),
                Step::Call {
                    tool: "exec_command".into(),
                    summary: String::new(),
                    input: "git status".into(),
                },
            ]
        );
    }

    #[test]
    fn pi_reads_thinking_and_tool_calls() {
        let got = steps(
            ClientKind::Pi,
            &[
                r#"{"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"toolCall","name":"bash","arguments":{"command":"grep x"}}]}}"#,
            ],
        );
        assert_eq!(
            got,
            vec![
                Step::Thinking("hmm".into()),
                Step::Call {
                    tool: "bash".into(),
                    summary: String::new(),
                    input: "grep x".into(),
                },
            ]
        );
    }
}
