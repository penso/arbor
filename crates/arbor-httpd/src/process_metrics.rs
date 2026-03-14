use {
    arbor_core::{
        daemon::{DaemonSessionRecord, TerminalDaemon, TerminalSessionState},
        process::ProcessInfo,
    },
    std::collections::{HashMap, HashSet},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessSnapshot {
    parent_pid: u32,
    memory_bytes: u64,
}

pub(crate) fn collect_session_memory_bytes<D>(daemon: &D) -> Result<HashMap<String, u64>, String>
where
    D: TerminalDaemon,
    D::Error: ToString,
{
    let sessions = daemon.list_sessions().map_err(|error| error.to_string())?;
    Ok(session_memory_bytes_from_sessions(
        &sessions,
        &list_process_snapshot(),
    ))
}

pub(crate) fn attach_process_memory(
    processes: &mut [ProcessInfo],
    session_memory_bytes: &HashMap<String, u64>,
) {
    for process in processes {
        process.memory_bytes = process
            .session_id
            .as_ref()
            .and_then(|session_id| session_memory_bytes.get(session_id).copied());
    }
}

fn session_memory_bytes_from_sessions(
    sessions: &[DaemonSessionRecord],
    process_snapshot: &HashMap<u32, ProcessSnapshot>,
) -> HashMap<String, u64> {
    let children_by_parent = build_children_by_parent(process_snapshot);
    let mut session_memory_bytes = HashMap::new();

    for session in sessions {
        if session.state != Some(TerminalSessionState::Running) {
            continue;
        }

        let Some(root_pid) = session.root_pid else {
            continue;
        };

        let Some(memory_bytes) =
            subtree_memory_bytes(root_pid, process_snapshot, &children_by_parent)
        else {
            continue;
        };

        session_memory_bytes.insert(session.session_id.to_string(), memory_bytes);
    }

    session_memory_bytes
}

fn build_children_by_parent(
    process_snapshot: &HashMap<u32, ProcessSnapshot>,
) -> HashMap<u32, Vec<u32>> {
    let mut children_by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, process) in process_snapshot {
        children_by_parent
            .entry(process.parent_pid)
            .or_default()
            .push(pid);
    }
    children_by_parent
}

fn subtree_memory_bytes(
    root_pid: u32,
    process_snapshot: &HashMap<u32, ProcessSnapshot>,
    children_by_parent: &HashMap<u32, Vec<u32>>,
) -> Option<u64> {
    let mut total_memory_bytes = 0_u64;
    let mut visited = HashSet::new();
    let mut stack = vec![root_pid];
    let mut found_process = false;

    // Sum the whole process tree so shell wrappers do not under-report memory.
    while let Some(pid) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }

        let Some(process) = process_snapshot.get(&pid) else {
            continue;
        };

        found_process = true;
        total_memory_bytes = total_memory_bytes.saturating_add(process.memory_bytes);

        if let Some(children) = children_by_parent.get(&pid) {
            stack.extend(children.iter().copied());
        }
    }

    found_process.then_some(total_memory_bytes)
}

#[cfg(unix)]
fn list_process_snapshot() -> HashMap<u32, ProcessSnapshot> {
    let output = match std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss="])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return HashMap::new(),
    };

    parse_unix_process_snapshot(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(windows)]
fn list_process_snapshot() -> HashMap<u32, ProcessSnapshot> {
    let output = match std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,WorkingSetSize | ConvertTo-Json -Compress",
        ])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return HashMap::new(),
    };

    parse_windows_process_snapshot(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(any(unix, windows)))]
fn list_process_snapshot() -> HashMap<u32, ProcessSnapshot> {
    HashMap::new()
}

#[cfg(unix)]
fn parse_unix_process_snapshot(output: &str) -> HashMap<u32, ProcessSnapshot> {
    let mut processes = HashMap::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let mut parts = trimmed.split_whitespace();
        let Some(pid) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Some(parent_pid) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Some(rss_kib) = parts.next().and_then(|value| value.parse::<u64>().ok()) else {
            continue;
        };

        processes.insert(pid, ProcessSnapshot {
            parent_pid,
            memory_bytes: rss_kib.saturating_mul(1024),
        });
    }

    processes
}

#[cfg(windows)]
fn parse_windows_process_snapshot(output: &str) -> HashMap<u32, ProcessSnapshot> {
    let parsed = match serde_json::from_str::<serde_json::Value>(output.trim()) {
        Ok(parsed) => parsed,
        Err(_) => return HashMap::new(),
    };
    let entries = match parsed {
        serde_json::Value::Array(values) => values,
        value => vec![value],
    };

    let mut processes = HashMap::new();
    for entry in entries {
        let Some(pid) = entry.get("ProcessId").and_then(|value| value.as_u64()) else {
            continue;
        };
        let Some(parent_pid) = entry
            .get("ParentProcessId")
            .and_then(|value| value.as_u64())
        else {
            continue;
        };
        let Some(memory_bytes) = entry.get("WorkingSetSize").and_then(|value| value.as_u64())
        else {
            continue;
        };

        processes.insert(pid as u32, ProcessSnapshot {
            parent_pid: parent_pid as u32,
            memory_bytes,
        });
    }

    processes
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        arbor_core::{SessionId, WorkspaceId, daemon::TerminalSessionState},
        std::path::PathBuf,
    };

    #[test]
    fn session_memory_sums_the_full_process_tree() {
        let process_snapshot = HashMap::from([
            (10, ProcessSnapshot {
                parent_pid: 1,
                memory_bytes: 4_096,
            }),
            (11, ProcessSnapshot {
                parent_pid: 10,
                memory_bytes: 8_192,
            }),
            (12, ProcessSnapshot {
                parent_pid: 11,
                memory_bytes: 16_384,
            }),
            (20, ProcessSnapshot {
                parent_pid: 1,
                memory_bytes: 32_768,
            }),
        ]);
        let sessions = vec![
            DaemonSessionRecord {
                session_id: SessionId::from("daemon-1"),
                workspace_id: WorkspaceId::from("/tmp/repo"),
                cwd: PathBuf::from("/tmp/repo"),
                shell: "/bin/zsh".to_owned(),
                root_pid: Some(10),
                cols: 120,
                rows: 35,
                title: None,
                last_command: None,
                output_tail: None,
                exit_code: None,
                state: Some(TerminalSessionState::Running),
                updated_at_unix_ms: None,
            },
            DaemonSessionRecord {
                session_id: SessionId::from("daemon-2"),
                workspace_id: WorkspaceId::from("/tmp/repo"),
                cwd: PathBuf::from("/tmp/repo"),
                shell: "/bin/zsh".to_owned(),
                root_pid: Some(20),
                cols: 120,
                rows: 35,
                title: None,
                last_command: None,
                output_tail: None,
                exit_code: None,
                state: Some(TerminalSessionState::Completed),
                updated_at_unix_ms: None,
            },
        ];

        let session_memory = session_memory_bytes_from_sessions(&sessions, &process_snapshot);

        assert_eq!(session_memory.get("daemon-1"), Some(&28_672));
        assert_eq!(session_memory.get("daemon-2"), None);
    }

    #[cfg(unix)]
    #[test]
    fn parses_unix_process_snapshot_rss_in_kib() {
        let snapshot = parse_unix_process_snapshot("10 1 256\n11 10 1024\n");

        assert_eq!(
            snapshot.get(&10),
            Some(&ProcessSnapshot {
                parent_pid: 1,
                memory_bytes: 262_144,
            })
        );
        assert_eq!(
            snapshot.get(&11),
            Some(&ProcessSnapshot {
                parent_pid: 10,
                memory_bytes: 1_048_576,
            })
        );
    }
}
