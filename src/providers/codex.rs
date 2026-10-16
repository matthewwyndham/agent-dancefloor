use crate::model::{
    ClientKind, ContextUsage, Detail, ProcessSnapshot, Session, Status, ToolCall, PROMPT_CHARS_MAX,
    SESSIONS_MAX, TRANSCRIPT_LINES_MAX,
};
use anyhow::Result;
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

fn is_codex(command: &str, args: &[String]) -> bool {
    let c = command.to_ascii_lowercase();
    let argv0 = args
        .first()
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // The Linux sandbox deliberately uses `codex` as its process title. It is
    // a tool subprocess, not another agent session. `comm` therefore cannot be
    // trusted without checking argv[0].
    if argv0.starts_with("codex-linux-sandbox") {
        return false;
    }

    c == "codex"
        || c == "codex.exe"
        || (c == "node"
            && args.iter().any(|arg| {
                Path::new(arg)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("codex.js"))
            }))
}

pub fn discover(home: &Path, processes: &ProcessSnapshot) -> Result<Vec<Session>> {
    let mut out = Vec::new();
    let mut used_rollouts = HashSet::new();
    let mut used_session_ids = HashSet::new();
    let candidates = processes
        .processes
        .iter()
        .filter(|p| is_codex(&p.command, &p.args))
        .take(SESSIONS_MAX)
        .collect::<Vec<_>>();
    let mut roots = candidates
        .iter()
        .copied()
        .filter(|process| {
            // The npm Node launcher waits on the native Codex child. Count that
            // process family once, using the native process for its resource data.
            !candidates.iter().any(|child| {
                child.ppid == Some(process.pid)
                    && process.command.eq_ignore_ascii_case("node")
                    && !child.command.eq_ignore_ascii_case("node")
            })
        })
        .collect::<Vec<_>>();
    // `ps` is PID-ordered, while transcripts are newest-first. Pair newest
    // processes first so concurrent sessions in one cwd are not swapped.
    roots.sort_by_key(|process| std::cmp::Reverse((process.started_at_ms, process.pid)));
    for process in roots {
        let path = explicit_rollout(home, process)
            .or_else(|| find_recent_rollout(home, process.cwd.as_deref(), &used_rollouts));
        // A Codex launcher and its runtime can both look like Codex in `ps`.
        // Once one process has claimed a rollout, do not show that same
        // persistent session a second time.
        if let Some(path) = &path {
            if !used_rollouts.insert(path.clone()) {
                continue;
            }
        }
        let parsed = path.as_deref().and_then(|p| parse_rollout_record(p).ok());
        // Guardian/risk-review threads have their own rollout beside the user
        // thread, often with a newer mtime and the same cwd. They are internal
        // implementation details, not dashboard sessions.
        if parsed.as_ref().is_some_and(|parsed| !parsed.primary) {
            continue;
        }
        let (id, detail, cwd, busy) = parsed
            .map(|parsed| (parsed.id, parsed.detail, parsed.cwd, parsed.busy))
            .unwrap_or_else(|| {
                (
                    format!("pid-{}", process.pid),
                    Detail::default(),
                    process.cwd.clone().unwrap_or_default(),
                    false,
                )
            });
        if !id.starts_with("pid-") && !used_session_ids.insert(id.clone()) {
            continue;
        }
        let status = if busy {
            Status::Busy
        } else if detail.activity.last_turn.is_some() {
            Status::Idle
        } else {
            Status::Other
        };
        out.push(Session {
            client: ClientKind::Codex,
            pid: process.pid,
            session_id: id,
            cwd,
            name: detail.title.clone().unwrap_or_else(|| "(Codex)".into()),
            status,
            version: String::new(),
            kind: String::new(),
            entrypoint: String::new(),
            started_at_ms: process.started_at_ms.unwrap_or(0),
            status_updated_at_ms: 0,
            proc: Some(process.proc_stat()),
            configured_model: None,
            detail,
        });
    }
    Ok(out)
}

fn explicit_rollout(home: &Path, p: &crate::model::ProcessInfo) -> Option<PathBuf> {
    p.args
        .iter()
        .find_map(|arg| {
            let path = Path::new(arg);
            let is_rollout = path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("rollout-"));
            (is_rollout && path.is_file()).then(|| path.to_path_buf())
        })
        .filter(|p| p.starts_with(home.join("sessions")))
}

fn bounded_prompt(text: &str) -> String {
    text.chars().take(PROMPT_CHARS_MAX).collect()
}

fn find_recent_rollout(
    home: &Path,
    cwd: Option<&Path>,
    used_rollouts: &HashSet<PathBuf>,
) -> Option<PathBuf> {
    let root = home.join("sessions");
    let mut candidates = Vec::new();
    for year in std::fs::read_dir(root).ok()?.flatten().take(3) {
        for month in std::fs::read_dir(year.path()).ok()?.flatten().take(12) {
            for day in std::fs::read_dir(month.path()).ok()?.flatten().take(31) {
                for entry in std::fs::read_dir(day.path()).ok()?.flatten().take(128) {
                    let p = entry.path();
                    if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                        candidates.push(p);
                    }
                }
            }
        }
    }
    candidates.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    candidates.into_iter().rev().find(|p| {
        !used_rollouts.contains(p)
            && rollout_identity(p).is_some_and(|identity| {
                identity.primary && cwd.map(|c| identity.cwd == c).unwrap_or(true)
            })
    })
}

#[derive(Debug)]
struct RolloutIdentity {
    id: Option<String>,
    cwd: PathBuf,
    primary: bool,
}

fn rollout_identity(path: &Path) -> Option<RolloutIdentity> {
    let file = std::fs::File::open(path).ok()?;
    BufReader::new(file).lines().take(64).find_map(|line| {
        let v = serde_json::from_str::<Value>(&line.ok()?).ok()?;
        (v.get("type").and_then(Value::as_str) == Some("session_meta")).then(|| {
            let payload = v.get("payload").unwrap_or(&Value::Null);
            RolloutIdentity {
                id: payload
                    .get("id")
                    .or_else(|| payload.get("session_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                cwd: payload
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .unwrap_or_default(),
                primary: is_primary_thread(payload),
            }
        })
    })
}

fn is_primary_thread(payload: &Value) -> bool {
    let thread_source = payload.get("thread_source").and_then(Value::as_str);
    let source_is_subagent = payload
        .get("source")
        .and_then(|source| source.get("subagent"))
        .is_some();
    !source_is_subagent && !matches!(thread_source, Some(source) if source != "user")
}

#[derive(Debug)]
struct ParsedRollout {
    id: String,
    detail: Detail,
    cwd: PathBuf,
    primary: bool,
    busy: bool,
}

fn parse_rollout_record(path: &Path) -> Result<ParsedRollout> {
    let text = std::fs::read_to_string(path)?;
    let identity = text.lines().take(64).find_map(|line| {
        let v = serde_json::from_str::<Value>(line).ok()?;
        (v.get("type").and_then(Value::as_str) == Some("session_meta")).then(|| {
            let payload = v.get("payload").unwrap_or(&Value::Null);
            RolloutIdentity {
                id: payload
                    .get("id")
                    .or_else(|| payload.get("session_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                cwd: payload
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .unwrap_or_default(),
                primary: is_primary_thread(payload),
            }
        })
    });
    let mut id = identity.as_ref().and_then(|identity| identity.id.clone());
    let mut cwd = identity
        .as_ref()
        .map(|identity| identity.cwd.clone())
        .unwrap_or_default();
    let mut primary = identity.is_none_or(|identity| identity.primary);
    let mut busy = false;
    let mut detail = Detail {
        transcript: Some(path.to_path_buf()),
        ..Default::default()
    };
    let mut lines = text
        .lines()
        .rev()
        .take(TRANSCRIPT_LINES_MAX)
        .collect::<Vec<_>>();
    lines.reverse();
    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "session_meta" {
            let payload = v.get("payload").unwrap_or(&Value::Null);
            id = v
                .get("payload")
                .and_then(|x| x.get("id").or_else(|| x.get("session_id")))
                .and_then(Value::as_str)
                .map(str::to_owned);
            cwd = v
                .get("payload")
                .and_then(|x| x.get("cwd"))
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_default();
            primary = is_primary_thread(payload);
        }
        if kind == "turn_context" {
            detail.model = v
                .pointer("/payload/model")
                .and_then(Value::as_str)
                .map(str::to_owned);
            detail.effort = v
                .pointer("/payload/effort")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if kind == "event_msg" {
            let event = v.get("payload").unwrap_or(&Value::Null);
            match event.get("type").and_then(Value::as_str).unwrap_or("") {
                "task_started" => {
                    busy = true;
                    detail.activity.last_turn = None;
                }
                "task_complete" | "turn_aborted" => {
                    busy = false;
                    detail.activity.last_turn = Some(crate::model::Turn {
                        duration_ms: 0,
                        messages: 0,
                    })
                }
                _ => {}
            }
        }
        if kind == "response_item" {
            let p = v.get("payload").unwrap_or(&Value::Null);
            if p.get("type").and_then(Value::as_str) == Some("message")
                && p.get("role").and_then(Value::as_str) == Some("user")
            {
                let text = p
                    .get("content")
                    .and_then(Value::as_array)
                    .and_then(|a| {
                        a.iter()
                            .rev()
                            .find(|x| x.get("type").and_then(Value::as_str) == Some("input_text"))
                            .and_then(|x| x.get("text").and_then(Value::as_str))
                    })
                    .map(bounded_prompt);
                if text.is_some() {
                    detail.last_prompt = text;
                }
            }
            if matches!(
                p.get("type").and_then(Value::as_str),
                Some("function_call" | "custom_tool_call")
            ) {
                detail.activity.record_tool(ToolCall {
                    name: p
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .into(),
                    summary: String::new(),
                    detail: p
                        .get("arguments")
                        .or_else(|| p.get("input"))
                        .map(|value| match value {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        })
                        .unwrap_or_default(),
                });
            }
        }
        if kind == "event_msg"
            && v.pointer("/payload/type").and_then(Value::as_str) == Some("token_count")
        {
            if let Some(u) = v
                .pointer("/payload/info/last_token_usage")
                .or_else(|| v.pointer("/payload/info/total_token_usage"))
            {
                let input = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                let cached = u
                    .get("cached_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let usage = ContextUsage {
                    input: input.saturating_sub(cached),
                    output: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
                    cache_read: cached,
                    cache_creation: 0,
                };
                detail.usage_peak = detail.usage_peak.max(usage.total());
                detail.usage = Some(usage);
            }
        }
    }
    Ok(ParsedRollout {
        id: id.unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into()
        }),
        detail,
        cwd,
        primary,
        busy,
    })
}

pub fn parse_rollout(path: &Path) -> Result<(String, Detail, PathBuf)> {
    parse_rollout_record(path).map(|parsed| (parsed.id, parsed.detail, parsed.cwd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProcessInfo, ProcessSnapshot};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "dancefloor-codex-{}-{label}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn process(pid: u32, command: &str, argv0: &str, cwd: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid: None,
            started_at_ms: Some(1),
            elapsed_secs: Some(1),
            rss_kib: 1,
            cpu_percent: 0.0,
            executable: command.into(),
            command: command.into(),
            args: vec![argv0.into()],
            cwd: Some(PathBuf::from(cwd)),
        }
    }

    #[test]
    fn launcher_and_runtime_claiming_one_rollout_make_one_row() {
        let root = test_root("process-family");
        let path = root.join("sessions/2026/01/01/rollout-test.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"same-session","cwd":"/repo"}}
"#,
        )
        .unwrap();
        let process = |pid| ProcessInfo {
            pid,
            ppid: None,
            started_at_ms: None,
            elapsed_secs: None,
            rss_kib: 1,
            cpu_percent: 0.0,
            executable: "codex".into(),
            command: "codex".into(),
            args: vec![path.to_string_lossy().into_owned()],
            cwd: Some(PathBuf::from("/repo")),
        };
        let sessions = discover(
            &root,
            &ProcessSnapshot {
                processes: vec![process(1), process(2)],
            },
        )
        .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "same-session");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn sandbox_tool_process_is_not_a_session() {
        let root = test_root("sandbox");
        let snapshot = ProcessSnapshot {
            processes: vec![process(7, "codex", "codex-linux-sandbox", "/repo")],
        };

        assert!(discover(&root, &snapshot).unwrap().is_empty());
    }

    #[test]
    fn guardian_rollout_cannot_replace_the_user_session() {
        let root = test_root("guardian");
        let dir = root.join("sessions/2026/09/10");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rollout-user.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"user-session","cwd":"/repo","source":"cli","thread_source":"user"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the bug"}]}}
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("rollout-guardian.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"guardian","cwd":"/repo","source":{"subagent":{"other":"guardian"}},"thread_source":"guardian_review"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"{\"risk_level\":\"low\"}"}]}}
"#,
        )
        .unwrap();
        let snapshot = ProcessSnapshot {
            processes: vec![process(8, "codex", "codex", "/repo")],
        };

        let sessions = discover(&root, &snapshot).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "user-session");
        assert_eq!(
            sessions[0].detail.last_prompt.as_deref(),
            Some("fix the bug")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn parser_keeps_only_human_prompts_and_reads_current_events() {
        let root = test_root("schema");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"session","cwd":"/repo","thread_source":"user"}}
{"type":"event_msg","payload":{"type":"task_started"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"the real prompt"}]}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"{\"risk_level\":\"low\"}"}]}}
{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":70,"output_tokens":5}}}}
{"type":"event_msg","payload":{"type":"task_complete"}}
"#,
        )
        .unwrap();

        let parsed = parse_rollout_record(&path).unwrap();
        assert_eq!(
            parsed.detail.last_prompt.as_deref(),
            Some("the real prompt")
        );
        assert!(!parsed.busy);
        assert_eq!(parsed.detail.usage.unwrap().total(), 105);
        std::fs::remove_dir_all(root).ok();
    }
}
