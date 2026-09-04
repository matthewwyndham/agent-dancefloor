//! Puts text on the system clipboard by handing it to whichever helper the
//! machine has, the same way session stats come from `ps` rather than a crate.

use std::io::Write;
use std::process::{Command, Stdio};

/// Wayland first, then X11, then macOS. The first one that runs wins.
const HELPERS: [(&str, &[&str]); 4] = [
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
    ("pbcopy", &[]),
];

/// Returns the error to show the user, or None once the text is on the board.
pub fn copy(text: &str) -> Option<String> {
    for (program, args) in HELPERS {
        match feed(program, args, text) {
            Ok(true) => return None,
            // The helper ran and refused. Another one is unlikely to do better.
            Ok(false) => return Some(format!("{program} could not take the text")),
            Err(_) => continue,
        }
    }
    Some("no clipboard helper found (wl-copy, xclip, xsel, pbcopy)".to_string())
}

fn feed(program: &str, args: &[&str], text: &str) -> std::io::Result<bool> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(text.as_bytes())?;
    Ok(child.wait()?.success())
}
