//! Opens a file in the user's editor.

use std::path::Path;
use std::process::Command;

/// `$VISUAL`, then `$EDITOR`, then `vi`.
fn editor_command() -> Vec<String> {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| {
            value
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .find(|parts| !parts.is_empty())
        .unwrap_or_else(|| vec!["vi".to_string()])
}

/// Returns the error to show, or None.
pub fn open(path: &Path) -> Option<String> {
    let parts = editor_command();
    let (program, args) = parts.split_first()?;
    match Command::new(program).args(args).arg(path).status() {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!("{program} exited with {status}")),
        Err(error) => Some(format!("cannot run {program}: {error}")),
    }
}
