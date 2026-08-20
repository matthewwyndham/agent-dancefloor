//! Reads the subagents a session has spawned.
//!
//! They live beside the transcript in `<session_id>/subagents/`, as an
//! `agent-<id>.meta.json` describing each one and an `agent-<id>.jsonl` holding
//! its conversation.

use std::path::Path;
use std::time::SystemTime;

use serde::Deserialize;

use crate::model::{Subagent, SUBAGENTS_MAX};

#[derive(Debug, Deserialize)]
struct MetaFile {
    #[serde(default, rename = "agentType")]
    agent_type: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "spawnDepth")]
    spawn_depth: Option<u64>,
}

/// `transcript` is the session's own `.jsonl`; its sibling directory of the same
/// stem holds the subagents.
pub fn read(transcript: &Path) -> Vec<Subagent> {
    let Some(stem) = transcript.file_stem() else {
        return Vec::new();
    };
    let Some(parent) = transcript.parent() else {
        return Vec::new();
    };
    let dir = parent.join(stem).join("subagents");

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut agents = Vec::new();
    for entry in entries.take(SUBAGENTS_MAX * 4) {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !filename.ends_with(".meta.json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<MetaFile>(&text) else {
            continue;
        };

        // Size and age come from the conversation file, not the metadata: they
        // are what show whether an agent is still working.
        let conversation = path.with_file_name(filename.replace(".meta.json", ".jsonl"));
        let (bytes, age_secs) = match std::fs::metadata(&conversation) {
            Ok(meta) => (
                meta.len(),
                meta.modified()
                    .ok()
                    .and_then(|m| SystemTime::now().duration_since(m).ok())
                    .map(|d| d.as_secs()),
            ),
            Err(_) => (0, None),
        };

        agents.push(Subagent {
            name: meta.name.unwrap_or_else(|| "(unnamed)".to_string()),
            agent_type: meta.agent_type.unwrap_or_default(),
            description: meta.description.unwrap_or_default(),
            spawn_depth: meta.spawn_depth.unwrap_or(0),
            age_secs,
            bytes,
        });

        if agents.len() >= SUBAGENTS_MAX {
            break;
        }
    }

    // Most recently active first, so a running agent is never below a finished one.
    agents.sort_by_key(|a| a.age_secs.unwrap_or(u64::MAX));
    agents
}
