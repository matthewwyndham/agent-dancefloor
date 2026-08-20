//! A long session pushes the human's prompt out of the tail window. Before the
//! backward search existed, the Prompt pane reported "none found" for any session
//! whose current turn ran more than a tail's worth of tool calls — which is every
//! long session. Shrinking the search back to a single tail read fails this.

use std::io::Write;

use dancefloor::model::TRANSCRIPT_TAIL_BYTES_MAX;
use dancefloor::transcript;

fn write_transcript(path: &std::path::Path, filler_bytes: u64) {
    let mut file = std::fs::File::create(path).expect("create");

    writeln!(
        file,
        r#"{{"type":"user","uuid":"u1","message":{{"role":"user","content":"the buried prompt"}}}}"#
    )
    .unwrap();

    // Tool results are user-role entries, so they fill the window without ever
    // looking like a prompt.
    let payload = "y".repeat(900);
    let line = format!(
        r#"{{"type":"user","uuid":"t","message":{{"role":"user","content":[{{"type":"tool_result","content":"{payload}"}}]}}}}"#
    );
    let mut written = 0u64;
    while written < filler_bytes {
        writeln!(file, "{line}").unwrap();
        written += line.len() as u64 + 1;
    }

    writeln!(
        file,
        r#"{{"type":"assistant","message":{{"model":"claude-opus-5","usage":{{"input_tokens":1,"cache_read_input_tokens":9,"output_tokens":3}}}}}}"#
    )
    .unwrap();
    writeln!(file, r#"{{"type":"last-prompt","leafUuid":"u1"}}"#).unwrap();
}

#[test]
fn finds_a_prompt_that_fell_out_of_the_tail_window() {
    let dir = std::env::temp_dir().join("dancefloor-long-transcript");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("session.jsonl");

    // Comfortably past the tail, so the prompt cannot be in the first read.
    write_transcript(&path, TRANSCRIPT_TAIL_BYTES_MAX + 512 * 1024);

    let detail = transcript::read(&path);
    assert_eq!(
        detail.last_prompt.as_deref(),
        Some("the buried prompt"),
        "prompt was not recovered from beyond the tail window"
    );
    // The newest usage block still has to come from the tail read.
    assert_eq!(detail.usage.expect("usage").total(), 13);

    std::fs::remove_dir_all(&dir).ok();
}
