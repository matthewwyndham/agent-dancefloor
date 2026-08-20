//! Finds live sessions. `~/.claude/sessions/<pid>.json` is the registry every
//! running Claude Code process maintains; a file outlives its process, so each
//! entry is confirmed against `ps` before it is reported.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::model::{ProcStat, Session, Status, SESSIONS_MAX};

#[derive(Debug, Deserialize)]
struct SessionFile {
    pid: u32,
    #[serde(rename = "sessionId")]
    session_id: String,
    cwd: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    entrypoint: Option<String>,
    #[serde(default, rename = "startedAt")]
    started_at: Option<i64>,
    #[serde(default, rename = "statusUpdatedAt")]
    status_updated_at: Option<i64>,
}

pub fn claude_home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".claude"))
}

/// Read the registry and return only sessions whose process is still alive.
pub fn scan(claude_home: &Path) -> Result<Vec<Session>> {
    let dir = claude_home.join("sessions");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        // No sessions directory at all is a normal state, not a failure: it just
        // means Claude Code has never run as this user.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).context(format!("read {}", dir.display())),
    };

    let mut files: Vec<SessionFile> = Vec::new();
    for entry in entries.take(SESSIONS_MAX * 4) {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // A session mid-write yields a partial file. Skip it; the next refresh
        // picks it up.
        if let Ok(parsed) = serde_json::from_str::<SessionFile>(&text) {
            files.push(parsed);
        }
        if files.len() >= SESSIONS_MAX {
            break;
        }
    }

    let pids: Vec<u32> = files.iter().map(|f| f.pid).collect();
    let alive = probe_processes(&pids);

    let mut sessions = Vec::new();
    for file in files {
        let Some(stat) = alive.get(&file.pid) else {
            continue;
        };
        sessions.push(Session {
            pid: file.pid,
            session_id: file.session_id,
            cwd: PathBuf::from(file.cwd),
            name: file.name.unwrap_or_else(|| "(unnamed)".to_string()),
            status: Status::parse(file.status.as_deref().unwrap_or("")),
            version: file.version.unwrap_or_default(),
            kind: file.kind.unwrap_or_default(),
            entrypoint: file.entrypoint.unwrap_or_default(),
            started_at_ms: file.started_at.unwrap_or(0),
            status_updated_at_ms: file.status_updated_at.unwrap_or(0),
            proc: Some(stat.clone()),
            detail: Default::default(),
        });
    }
    Ok(sessions)
}

/// One `ps` call for every candidate pid. Returns an entry only for pids that
/// are alive AND still running `claude`, which is what rules out a registry file
/// left behind by a crash whose pid the OS has since handed to something else.
fn probe_processes(pids: &[u32]) -> HashMap<u32, ProcStat> {
    let mut out = HashMap::new();
    if pids.is_empty() {
        return out;
    }

    let list = pids
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",");

    // `ps` exits non-zero when no pid matches, which is an ordinary outcome here.
    let Ok(result) = Command::new("ps")
        .args(["-o", "pid=,rss=,pcpu=,comm=", "-p", &list])
        .output()
    else {
        return out;
    };

    let text = String::from_utf8_lossy(&result.stdout);
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(rss), Some(cpu), Some(comm)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let basename = comm.rsplit('/').next().unwrap_or(comm);
        if basename != "claude" {
            continue;
        }
        let (Ok(pid), Ok(rss_kib), Ok(cpu_percent)) =
            (pid.parse::<u32>(), rss.parse::<u64>(), cpu.parse::<f64>())
        else {
            continue;
        };
        out.insert(
            pid,
            ProcStat {
                rss_kib,
                cpu_percent,
            },
        );
    }
    out
}
