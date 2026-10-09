//! One bounded, injectable process snapshot per refresh.

use std::path::PathBuf;
use std::process::Command;

use crate::model::{ProcessInfo, ProcessSnapshot};

pub fn snapshot() -> ProcessSnapshot {
    let Ok(output) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,etime=,rss=,pcpu=,comm=,args="])
        .output()
    else {
        return ProcessSnapshot::default();
    };
    parse_snapshot(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_snapshot(text: &str) -> ProcessSnapshot {
    let mut processes = Vec::new();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok());
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(elapsed), Some(rss), Some(cpu), Some(executable)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let Ok(pid) = pid.parse() else { continue };
        let args = fields.map(str::to_owned).collect::<Vec<_>>();
        let elapsed_secs = parse_elapsed(elapsed);
        let started_at_ms = now_ms.zip(elapsed_secs).map(|(now, elapsed)| {
            now.saturating_sub(
                i64::try_from(elapsed)
                    .unwrap_or(i64::MAX)
                    .saturating_mul(1000),
            )
        });
        processes.push(ProcessInfo {
            pid,
            ppid: ppid.parse().ok(),
            started_at_ms,
            elapsed_secs,
            rss_kib: rss.parse().unwrap_or(0),
            cpu_percent: cpu.parse().unwrap_or(0.0),
            executable: executable.to_owned(),
            command: executable
                .rsplit('/')
                .next()
                .unwrap_or(executable)
                .to_owned(),
            args,
            cwd: cwd(pid),
        });
    }
    ProcessSnapshot { processes }
}

fn parse_elapsed(value: &str) -> Option<u64> {
    let parts: Vec<_> = value.split('-').collect();
    let (days, clock) = if parts.len() == 2 {
        (parts[0].parse().ok()?, parts[1])
    } else {
        (0, parts[0])
    };
    let nums: Vec<_> = clock.split(':').map(|x| x.parse::<u64>().ok()).collect();
    let (hours, minutes, seconds) = match nums.as_slice() {
        [Some(m), Some(s)] => (0, *m, *s),
        [Some(h), Some(m), Some(s)] => (*h, *m, *s),
        _ => return None,
    };
    Some(days * 86400 + hours * 3600 + minutes * 60 + seconds)
}

fn cwd(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_elapsed_times() {
        assert_eq!(parse_elapsed("1-02:03:04"), Some(93_784));
        assert_eq!(parse_elapsed("02:03"), Some(123));
    }
}
