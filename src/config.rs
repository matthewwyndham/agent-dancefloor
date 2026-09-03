//! dancefloor's own config file.
//!
//! A running session records nothing about the `[1m]` variant — the model id on
//! an assistant message is the base id either way, and the `cost-state` line
//! that names the variant is only written at shutdown. So a machine whose
//! sessions are long-context by default cannot be detected, and this file is
//! where the user says so.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::tokens_parse;

/// Where the file sits under the config home.
const FILE: &str = "dancefloor/config.json";

/// `$XDG_CONFIG_HOME`, or `~/.config`. Neither being set is the same answer as
/// a missing file: nothing configured.
pub fn path() -> Option<PathBuf> {
    let home = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(home.join(FILE))
}

/// The window to assume for a session that gives away nothing about its own.
///
/// Written either as a number or in the shorthand the flags take, so `1000000`
/// and `"1m"` mean the same thing.
///
/// A missing, unreadable or half-written file is no answer rather than a
/// failure. Zero is refused as well, because a zero window pegs every gauge at
/// 0% and reads as a working display.
pub fn default_context_limit(path: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    match value.get("default_context_limit")? {
        Value::String(raw) => tokens_parse(raw),
        other => other.as_u64().filter(|n| *n > 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(dir).join(FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    /// The contract with the file on disk: this key, either spelling of the
    /// value. Rename the key and a config that looks right stops being read.
    #[test]
    fn a_configured_limit_is_read_as_a_number_or_the_shorthand() {
        for body in [
            r#"{"default_context_limit": 1000000}"#,
            r#"{"default_context_limit": "1m"}"#,
        ] {
            let path = write("dancefloor-config-read", body);
            assert_eq!(
                default_context_limit(&path),
                Some(1_000_000),
                "body: {body}"
            );
        }
        std::fs::remove_dir_all(std::env::temp_dir().join("dancefloor-config-read")).ok();
    }

    #[test]
    fn a_useless_value_is_no_answer_rather_than_a_failure() {
        for body in [
            r#"{"default_context_limit": 0}"#,
            r#"{"default_context_limit": "wide"}"#,
            r#"{"default_context_limit": -5}"#,
            r#"{"interval": 4}"#,
            "{ truncated",
        ] {
            let path = write("dancefloor-config-junk", body);
            assert_eq!(default_context_limit(&path), None, "body: {body}");
        }
        let absent = std::env::temp_dir().join("dancefloor-config-absent.json");
        std::fs::remove_file(&absent).ok();
        assert_eq!(default_context_limit(&absent), None);
        std::fs::remove_dir_all(std::env::temp_dir().join("dancefloor-config-junk")).ok();
    }
}
