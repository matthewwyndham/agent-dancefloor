use crate::model::{
    ClientKind, ContextUsage, Detail, ProcessSnapshot, Session, Status, ToolCall, PROMPT_CHARS_MAX,
    SESSIONS_MAX,
};
use anyhow::Result;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

fn is_pi(command: &str) -> bool {
    matches!(command.to_ascii_lowercase().as_str(), "pi" | "pi-rpc")
}

pub fn discover(sessions_dir: &Path, processes: &ProcessSnapshot) -> Result<Vec<Session>> {
    let files = session_files(sessions_dir);
    let mut out = Vec::new();
    let mut used_files = HashSet::new();
    let mut used_ids = HashSet::new();
    let mut candidates = processes
        .processes
        .iter()
        .filter(|p| is_pi(&p.command))
        .take(SESSIONS_MAX)
        .collect::<Vec<_>>();
    candidates.sort_by_key(|process| std::cmp::Reverse((process.started_at_ms, process.pid)));
    for p in candidates {
        let path = if has_flag(&p.args, "--no-session") {
            None
        } else {
            explicit_session(&p.args, &files).or_else(|| {
                files
                    .iter()
                    .find(|f| {
                        !used_files.contains(*f)
                            && session_identity(f).is_some_and(|identity| {
                                identity.primary
                                    && p.cwd
                                        .as_deref()
                                        .map(|cwd| identity.cwd == cwd)
                                        .unwrap_or(false)
                            })
                    })
                    .cloned()
            })
        };
        if let Some(path) = &path {
            if !used_files.insert(path.clone()) {
                continue;
            }
        }
        let parsed = path.as_deref().and_then(|x| parse_session_record(x).ok());
        if parsed.as_ref().is_some_and(|parsed| !parsed.primary) {
            continue;
        }
        let (id, detail, cwd, idle) = parsed
            .map(|parsed| (parsed.id, parsed.detail, parsed.cwd, parsed.idle))
            .unwrap_or_else(|| {
                (
                    format!("pid-{}", p.pid),
                    Detail::default(),
                    p.cwd.clone().unwrap_or_default(),
                    false,
                )
            });
        if !id.starts_with("pid-") && !used_ids.insert(id.clone()) {
            continue;
        }
        let status = if idle {
            Status::Idle
        } else if detail.last_prompt.is_some() || !detail.activity.tools.is_empty() {
            Status::Busy
        } else {
            Status::Other
        };
        out.push(Session {
            client: ClientKind::Pi,
            pid: p.pid,
            session_id: id,
            cwd,
            name: detail.title.clone().unwrap_or_else(|| "(Pi)".into()),
            status,
            version: String::new(),
            kind: String::new(),
            entrypoint: String::new(),
            started_at_ms: p.started_at_ms.unwrap_or(0),
            status_updated_at_ms: 0,
            proc: Some(p.proc_stat()),
            configured_model: None,
            detail,
        });
    }
    Ok(out)
}

fn session_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(p: &Path, o: &mut Vec<PathBuf>) {
        if o.len() >= SESSIONS_MAX * 4 {
            return;
        }
        if let Ok(es) = std::fs::read_dir(p) {
            for e in es.flatten() {
                let x = e.path();
                if x.is_dir() {
                    walk(&x, o)
                } else if x.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                    o.push(x)
                }
            }
        }
    }
    walk(root, &mut out);
    out.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
    });
    out.reverse();
    out
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn explicit_session(args: &[String], files: &[PathBuf]) -> Option<PathBuf> {
    let value = args
        .windows(2)
        .find_map(|pair| {
            matches!(pair[0].as_str(), "--session" | "--session-file").then_some(pair[1].as_str())
        })
        .or_else(|| {
            args.iter().find_map(|arg| {
                arg.strip_prefix("--session=")
                    .or_else(|| arg.strip_prefix("--session-file="))
            })
        })?;
    let path = Path::new(value);
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    files
        .iter()
        .find(|file| {
            file.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem == value || stem.ends_with(&format!("_{value}")))
        })
        .cloned()
}

#[derive(Debug)]
struct SessionIdentity {
    cwd: PathBuf,
    primary: bool,
}

fn session_identity(path: &Path) -> Option<SessionIdentity> {
    let file = std::fs::File::open(path).ok()?;
    BufReader::new(file).lines().take(64).find_map(|line| {
        let value = serde_json::from_str::<Value>(&line.ok()?).ok()?;
        (value.get("type").and_then(Value::as_str) == Some("session")).then(|| SessionIdentity {
            cwd: value
                .get("cwd")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_default(),
            primary: value.get("parentSession").is_none_or(Value::is_null),
        })
    })
}

#[derive(Debug)]
struct ParsedSession {
    id: String,
    detail: Detail,
    cwd: PathBuf,
    primary: bool,
    idle: bool,
}

fn text_content(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.chars().take(PROMPT_CHARS_MAX).collect());
    }
    let parts = content
        .as_array()?
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n").chars().take(PROMPT_CHARS_MAX).collect())
}

fn active_path(entries: &[Value]) -> Vec<&Value> {
    let body = entries
        .iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) != Some("session"))
        .collect::<Vec<_>>();
    let by_id = body
        .iter()
        .filter_map(|entry| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id, *entry))
        })
        .collect::<HashMap<_, _>>();
    let Some(mut current) = body.last().copied() else {
        return Vec::new();
    };
    if by_id.is_empty() {
        return body;
    }
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    while let Some(id) = current.get("id").and_then(Value::as_str) {
        if !seen.insert(id) {
            break;
        }
        path.push(current);
        let Some(parent) = current.get("parentId").and_then(Value::as_str) else {
            break;
        };
        let Some(next) = by_id.get(parent) else {
            break;
        };
        current = next;
    }
    path.reverse();
    path
}

fn parse_session_record(path: &Path) -> Result<ParsedSession> {
    let text = std::fs::read_to_string(path)?;
    let mut id: String = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut cwd = PathBuf::new();
    let mut detail = Detail {
        transcript: Some(path.to_path_buf()),
        ..Default::default()
    };
    let entries = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    let mut primary = true;
    if let Some(header) = entries
        .iter()
        .find(|entry| entry.get("type").and_then(Value::as_str) == Some("session"))
    {
        id = header
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_owned();
        cwd = header
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_default();
        primary = header.get("parentSession").is_none_or(Value::is_null);
    }
    let path_entries = active_path(&entries);
    for v in &path_entries {
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "session_info" => {
                detail.title = v
                    .get("name")
                    .or_else(|| v.get("title"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            "message" => {
                let message = v.get("message").unwrap_or(&Value::Null);
                match message.get("role").and_then(Value::as_str).unwrap_or("") {
                    "user" => {
                        detail.last_prompt = message.get("content").and_then(text_content);
                    }
                    "assistant" => {
                        detail.model = message
                            .get("model")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .or(detail.model);
                        if let Some(usage) = message.get("usage") {
                            let context = ContextUsage {
                                input: usage.get("input").and_then(Value::as_u64).unwrap_or(0),
                                output: usage.get("output").and_then(Value::as_u64).unwrap_or(0),
                                cache_read: usage
                                    .get("cacheRead")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0),
                                cache_creation: usage
                                    .get("cacheWrite")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0),
                            };
                            detail.usage_peak = detail.usage_peak.max(context.total());
                            detail.usage = Some(context);
                        }
                        if let Some(content) = message.get("content").and_then(Value::as_array) {
                            for call in content.iter().filter(|part| {
                                part.get("type").and_then(Value::as_str) == Some("toolCall")
                            }) {
                                detail.activity.record_tool(ToolCall {
                                    name: call
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or("tool")
                                        .to_owned(),
                                    summary: String::new(),
                                    detail: call
                                        .get("arguments")
                                        .map(Value::to_string)
                                        .unwrap_or_default(),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            "model_change" => {
                detail.model = v
                    .get("modelId")
                    .or_else(|| v.get("model"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }
            "thinking_level_change" => {
                detail.effort = v
                    .get("thinkingLevel")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }
            _ => {}
        }
    }
    let idle = path_entries
        .iter()
        .rev()
        .find_map(|entry| {
            (entry.get("type").and_then(Value::as_str) == Some("message")).then(|| {
                let message = entry.get("message").unwrap_or(&Value::Null);
                message.get("role").and_then(Value::as_str) == Some("assistant")
                    && !message
                        .get("content")
                        .and_then(Value::as_array)
                        .is_some_and(|content| {
                            content.iter().any(|part| {
                                part.get("type").and_then(Value::as_str) == Some("toolCall")
                            })
                        })
            })
        })
        .unwrap_or(false);
    Ok(ParsedSession {
        id,
        detail,
        cwd,
        primary,
        idle,
    })
}

pub fn parse_session(path: &Path) -> Result<(String, Detail, PathBuf)> {
    parse_session_record(path).map(|parsed| (parsed.id, parsed.detail, parsed.cwd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProcessInfo, ProcessSnapshot};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "dancefloor-pi-{}-{label}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn process(args: Vec<&str>) -> ProcessInfo {
        ProcessInfo {
            pid: 20,
            ppid: None,
            started_at_ms: Some(1),
            elapsed_secs: Some(1),
            rss_kib: 1,
            cpu_percent: 0.0,
            executable: "pi".into(),
            command: "pi".into(),
            args: args.into_iter().map(str::to_owned).collect(),
            cwd: Some(PathBuf::from("/repo")),
        }
    }

    fn write_current_session(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            r#"{"type":"session","version":3,"id":"pi-session","cwd":"/repo"}
{"type":"message","id":"u1","parentId":null,"message":{"role":"user","content":[{"type":"text","text":"first prompt"}]}}
{"type":"message","id":"a1","parentId":"u1","message":{"role":"assistant","model":"model-old","content":[{"type":"text","text":"answer"}]}}
{"type":"message","id":"discarded-u","parentId":"a1","message":{"role":"user","content":[{"type":"text","text":"abandoned prompt"}]}}
{"type":"message","id":"discarded-a","parentId":"discarded-u","message":{"role":"assistant","model":"wrong-model","content":[{"type":"text","text":"abandoned"}]}}
{"type":"message","id":"active-u","parentId":"a1","message":{"role":"user","content":[{"type":"text","text":"active prompt"}]}}
{"type":"message","id":"tool","parentId":"active-u","message":{"role":"assistant","model":"model-new","content":[{"type":"toolCall","name":"read","arguments":{"path":"README.md"}}]}}
{"type":"message","id":"result","parentId":"tool","message":{"role":"toolResult","content":[{"type":"text","text":"ok"}]}}
{"type":"message","id":"final","parentId":"result","message":{"role":"assistant","model":"model-new","usage":{"input":10,"output":5,"cacheRead":20,"cacheWrite":2},"content":[{"type":"text","text":"done"}]}}
{"type":"session_info","id":"name","parentId":"final","name":"Useful session"}
"#,
        )
        .unwrap();
    }

    #[test]
    fn current_schema_and_active_branch_are_parsed() {
        let root = test_root("schema");
        let path = root.join("session.jsonl");
        write_current_session(&path);

        let parsed = parse_session_record(&path).unwrap();
        assert_eq!(parsed.id, "pi-session");
        assert_eq!(parsed.cwd, PathBuf::from("/repo"));
        assert_eq!(parsed.detail.title.as_deref(), Some("Useful session"));
        assert_eq!(parsed.detail.last_prompt.as_deref(), Some("active prompt"));
        assert_eq!(parsed.detail.model.as_deref(), Some("model-new"));
        assert_eq!(parsed.detail.usage.unwrap().total(), 37);
        assert_eq!(parsed.detail.activity.tools.len(), 1);
        assert!(parsed.idle);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn discovery_ignores_child_sessions_with_the_same_cwd() {
        let root = test_root("child");
        let primary = root.join("primary.jsonl");
        write_current_session(&primary);
        std::fs::write(
            root.join("child.jsonl"),
            r#"{"type":"session","version":3,"id":"child","cwd":"/repo","parentSession":"pi-session"}
{"type":"message","id":"u","parentId":null,"message":{"role":"user","content":[{"type":"text","text":"delegated internals"}]}}
"#,
        )
        .unwrap();

        let sessions = discover(
            &root,
            &ProcessSnapshot {
                processes: vec![process(vec!["pi"])],
            },
        )
        .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "pi-session");
        assert_eq!(sessions[0].status, Status::Idle);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn no_session_flag_stays_ephemeral() {
        let root = test_root("ephemeral");
        write_current_session(&root.join("session.jsonl"));

        let sessions = discover(
            &root,
            &ProcessSnapshot {
                processes: vec![process(vec!["pi", "--no-session"])],
            },
        )
        .unwrap();
        assert_eq!(sessions[0].session_id, "pid-20");
        assert!(sessions[0].detail.last_prompt.is_none());
        std::fs::remove_dir_all(root).ok();
    }
}
