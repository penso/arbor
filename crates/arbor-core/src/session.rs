use std::{
    fs,
    io::{self, BufRead},
    path::{Path, PathBuf},
};

/// Extract the initial user prompt from the most recent Claude or Codex session
/// associated with `worktree_path`.
///
/// Tries Claude first (O(1) directory lookup), then falls back to scanning
/// Codex session files (date-ordered, up to 30 days back).
pub fn extract_agent_task(worktree_path: &Path) -> Option<String> {
    extract_claude_task(worktree_path).or_else(|| extract_codex_task(worktree_path))
}

/// Encode a worktree path the same way Claude CLI names its project dirs:
/// replace `/` and `.` with `-`.
fn claude_project_key(worktree_path: &Path) -> String {
    let s = worktree_path.to_string_lossy();
    s.chars()
        .map(|c| {
            if c == '/' || c == '.' {
                '-'
            } else {
                c
            }
        })
        .collect()
}

/// Look up the most recent `.jsonl` in `~/.claude/projects/{key}/` and extract
/// the first `"type": "user"` message content.
fn extract_claude_task(worktree_path: &Path) -> Option<String> {
    let home = home_dir()?;
    let key = claude_project_key(worktree_path);
    let project_dir = home.join(".claude").join("projects").join(&key);

    if !project_dir.is_dir() {
        return None;
    }

    // Find the most recently modified .jsonl file.
    let mut jsonl_files: Vec<_> = fs::read_dir(&project_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();

    jsonl_files.sort_by(|a, b| {
        let ma = a.metadata().and_then(|m| m.modified()).ok();
        let mb = b.metadata().and_then(|m| m.modified()).ok();
        mb.cmp(&ma) // newest first
    });

    let newest = jsonl_files.first()?;
    extract_claude_user_prompt(&newest.path())
}

/// Read through a Claude `.jsonl` session file and return the text of the first
/// `"type": "user"` entry.
fn extract_claude_user_prompt(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = io::BufReader::new(file);

    for line in reader.lines() {
        let line = line.ok()?;
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line).ok()?;
        if value.get("type").and_then(|v| v.as_str()) == Some("user") {
            if let Some(content) = value.pointer("/message/content").and_then(|v| v.as_str()) {
                return Some(truncate_prompt(content));
            }
            // Content might be an array of blocks.
            if let Some(blocks) = value.pointer("/message/content").and_then(|v| v.as_array()) {
                for block in blocks {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        return Some(truncate_prompt(text));
                    }
                }
            }
        }
    }
    None
}

/// Scan Codex session dirs (newest day first, up to 30 days) for a session
/// whose `session_meta.payload.cwd` matches `worktree_path`, and extract the
/// first `event_msg` with `payload.type == "user_message"`.
fn extract_codex_task(worktree_path: &Path) -> Option<String> {
    let home = home_dir()?;
    let sessions_root = home.join(".codex").join("sessions");
    if !sessions_root.is_dir() {
        return None;
    }

    let worktree_str = worktree_path.to_string_lossy();

    // Collect year/month/day dirs and sort newest first.
    let day_dirs = collect_day_dirs(&sessions_root, 30)?;

    for day_dir in day_dirs {
        let mut files: Vec<_> = fs::read_dir(&day_dir)
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
            .collect();

        // Sort newest first by filename (contains timestamp).
        files.sort_by_key(|e| std::cmp::Reverse(e.file_name()));

        for entry in files {
            if let Some(prompt) =
                extract_codex_user_prompt_if_matching(&entry.path(), &worktree_str)
            {
                return Some(prompt);
            }
        }
    }
    None
}

/// Collect all `YYYY/MM/DD` directories under `sessions_root`, sorted newest
/// first, limited to `max_days` entries.
fn collect_day_dirs(sessions_root: &Path, max_days: usize) -> Option<Vec<PathBuf>> {
    let mut year_dirs: Vec<_> = fs::read_dir(sessions_root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    year_dirs.sort_by(|a, b| b.cmp(a));

    let mut day_dirs = Vec::new();
    'outer: for year_dir in &year_dirs {
        let mut month_dirs: Vec<_> = fs::read_dir(year_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.path())
            .collect();
        month_dirs.sort_by(|a, b| b.cmp(a));

        for month_dir in &month_dirs {
            let mut days: Vec<_> = fs::read_dir(month_dir)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.path())
                .collect();
            days.sort_by(|a, b| b.cmp(a));

            for day in days {
                day_dirs.push(day);
                if day_dirs.len() >= max_days {
                    break 'outer;
                }
            }
        }
    }
    Some(day_dirs)
}

/// Check whether a Codex session file's `session_meta.payload.cwd` matches
/// `worktree_str`, and if so extract the first user message.
fn extract_codex_user_prompt_if_matching(path: &Path, worktree_str: &str) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = io::BufReader::new(file);

    let mut cwd_matches = false;
    for line in reader.lines() {
        let line = line.ok()?;
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line).ok()?;

        let line_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if line_type == "session_meta" {
            let cwd = value
                .pointer("/payload/cwd")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if cwd == worktree_str {
                cwd_matches = true;
            } else {
                return None; // Wrong session, stop early.
            }
        }

        if cwd_matches && line_type == "event_msg" {
            let payload_type = value
                .pointer("/payload/type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if payload_type == "user_message"
                && let Some(msg) = value.pointer("/payload/message").and_then(|v| v.as_str())
            {
                return Some(truncate_prompt(msg));
            }
        }
    }
    None
}

/// Take the first line of a prompt, capped at ~80 characters.
fn truncate_prompt(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or(text);
    if first_line.len() <= 80 {
        first_line.to_owned()
    } else {
        let mut end = 80;
        // Avoid splitting in the middle of a multi-byte char.
        while !first_line.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}...", &first_line[..end])
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn claude_project_key_encodes_correctly() {
        let path = Path::new("/Users/penso/.superset/worktrees/arbor/psychedelic-gravity");
        assert_eq!(
            claude_project_key(path),
            "-Users-penso--superset-worktrees-arbor-psychedelic-gravity"
        );
    }

    #[test]
    fn claude_project_key_handles_dots() {
        let path = Path::new("/home/user/.config/project");
        assert_eq!(claude_project_key(path), "-home-user--config-project");
    }

    #[test]
    fn truncate_prompt_short_text() {
        assert_eq!(truncate_prompt("fix the bug"), "fix the bug");
    }

    #[test]
    fn truncate_prompt_multiline() {
        let text = "first line\nsecond line\nthird line";
        assert_eq!(truncate_prompt(text), "first line");
    }

    #[test]
    fn truncate_prompt_long_text() {
        let long = "a".repeat(100);
        let result = truncate_prompt(&long);
        assert!(result.ends_with("..."));
        // 80 chars + "..."
        assert_eq!(result.len(), 83);
    }

    #[test]
    fn truncate_prompt_exactly_80() {
        let text = "b".repeat(80);
        assert_eq!(truncate_prompt(&text), text);
    }

    #[test]
    fn extract_claude_user_prompt_parses_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test-session.jsonl");
        let content = r#"{"type":"file-history-snapshot","messageId":"abc"}
{"type":"user","message":{"role":"user","content":"fix the login bug"},"sessionId":"123"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"OK"}]}}
"#;
        fs::write(&file_path, content).unwrap();
        let result = extract_claude_user_prompt(&file_path);
        assert_eq!(result.as_deref(), Some("fix the login bug"));
    }

    #[test]
    fn extract_claude_user_prompt_array_content() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test-session.jsonl");
        let content = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"refactor auth"}]},"sessionId":"123"}
"#;
        fs::write(&file_path, content).unwrap();
        let result = extract_claude_user_prompt(&file_path);
        assert_eq!(result.as_deref(), Some("refactor auth"));
    }

    #[test]
    fn extract_codex_user_prompt_matching_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("session.jsonl");
        let content = r#"{"type":"session_meta","payload":{"cwd":"/repos/project","id":"abc"}}
{"type":"event_msg","payload":{"type":"task_started"}}
{"type":"event_msg","payload":{"type":"user_message","message":"add tests for parser"}}
"#;
        fs::write(&file_path, content).unwrap();
        let result = extract_codex_user_prompt_if_matching(&file_path, "/repos/project");
        assert_eq!(result.as_deref(), Some("add tests for parser"));
    }

    #[test]
    fn extract_codex_user_prompt_wrong_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("session.jsonl");
        let content = r#"{"type":"session_meta","payload":{"cwd":"/repos/other","id":"abc"}}
{"type":"event_msg","payload":{"type":"user_message","message":"add tests"}}
"#;
        fs::write(&file_path, content).unwrap();
        let result = extract_codex_user_prompt_if_matching(&file_path, "/repos/project");
        assert!(result.is_none());
    }
}
