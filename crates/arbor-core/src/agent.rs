use std::{collections::HashSet, path::PathBuf, process::Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Working,
    Waiting,
}

const AGENT_PROCESS_NAMES: &[&str] = &["claude", "codex", "opencode"];

/// Detect working directories of running AI tool processes.
///
/// Runs `pgrep` to find PIDs, then `lsof` to resolve their cwds.
pub fn detect_agent_cwds() -> HashSet<PathBuf> {
    let mut pids = Vec::new();
    for name in AGENT_PROCESS_NAMES {
        if let Ok(output) = Command::new("pgrep").arg("-x").arg(name).output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        pids.push(pid);
                    }
                }
            }
        }
    }

    if pids.is_empty() {
        return HashSet::new();
    }

    let pid_args: Vec<String> = pids.iter().map(|pid| pid.to_string()).collect();
    let mut lsof_args = vec!["-a", "-d", "cwd", "-F", "pn"];
    for pid_arg in &pid_args {
        lsof_args.push("-p");
        lsof_args.push(pid_arg);
    }

    let Ok(output) = Command::new("lsof").args(&lsof_args).output() else {
        return HashSet::new();
    };

    if !output.status.success() {
        return HashSet::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_lsof_output(&stdout)
}

/// Parse lsof `-F pn` output into a set of directory paths.
///
/// The format is lines starting with `p` (PID) or `n` (name/path).
/// We collect all `n`-prefixed lines as cwd paths.
pub fn parse_lsof_output(output: &str) -> HashSet<PathBuf> {
    let mut cwds = HashSet::new();
    for line in output.lines() {
        if let Some(path_str) = line.strip_prefix('n') {
            let path = PathBuf::from(path_str);
            if path.is_absolute() {
                cwds.insert(path);
            }
        }
    }
    cwds
}

/// Match detected agent cwds against worktree paths.
///
/// A cwd matches a worktree if the cwd is equal to or is a subdirectory of
/// the worktree path. When a cwd could match multiple worktrees (nested),
/// it matches the most specific (longest path) worktree.
pub fn worktrees_with_agents(
    agent_cwds: &HashSet<PathBuf>,
    worktree_paths: &[PathBuf],
) -> HashSet<PathBuf> {
    let mut matched = HashSet::new();

    for cwd in agent_cwds {
        let mut best_match: Option<&PathBuf> = None;
        for worktree_path in worktree_paths {
            if cwd.starts_with(worktree_path) {
                match best_match {
                    Some(current_best) => {
                        if worktree_path.as_os_str().len() > current_best.as_os_str().len() {
                            best_match = Some(worktree_path);
                        }
                    },
                    None => {
                        best_match = Some(worktree_path);
                    },
                }
            }
        }
        if let Some(worktree_path) = best_match {
            matched.insert(worktree_path.clone());
        }
    }

    matched
}

#[cfg(test)]
mod tests {
    use {super::*, std::path::Path};

    #[test]
    fn parse_lsof_output_extracts_paths() {
        let output = "p12345\nn/Users/dev/project\np67890\nn/Users/dev/other\n";
        let cwds = parse_lsof_output(output);
        assert_eq!(cwds.len(), 2);
        assert!(cwds.contains(Path::new("/Users/dev/project")));
        assert!(cwds.contains(Path::new("/Users/dev/other")));
    }

    #[test]
    fn parse_lsof_output_ignores_non_path_lines() {
        let output = "p12345\nf4\nn/Users/dev/project\n";
        let cwds = parse_lsof_output(output);
        assert_eq!(cwds.len(), 1);
        assert!(cwds.contains(Path::new("/Users/dev/project")));
    }

    #[test]
    fn parse_lsof_output_empty() {
        let cwds = parse_lsof_output("");
        assert!(cwds.is_empty());
    }

    #[test]
    fn parse_lsof_output_ignores_relative_paths() {
        let output = "p12345\nnrelative/path\nn/absolute/path\n";
        let cwds = parse_lsof_output(output);
        assert_eq!(cwds.len(), 1);
        assert!(cwds.contains(Path::new("/absolute/path")));
    }

    #[test]
    fn worktrees_with_agents_exact_match() {
        let cwds: HashSet<PathBuf> = [PathBuf::from("/repos/project")].into();
        let worktrees = vec![
            PathBuf::from("/repos/project"),
            PathBuf::from("/repos/other"),
        ];
        let matched = worktrees_with_agents(&cwds, &worktrees);
        assert_eq!(matched.len(), 1);
        assert!(matched.contains(Path::new("/repos/project")));
    }

    #[test]
    fn worktrees_with_agents_subdirectory_match() {
        let cwds: HashSet<PathBuf> = [PathBuf::from("/repos/project/src/lib")].into();
        let worktrees = vec![PathBuf::from("/repos/project")];
        let matched = worktrees_with_agents(&cwds, &worktrees);
        assert_eq!(matched.len(), 1);
        assert!(matched.contains(Path::new("/repos/project")));
    }

    #[test]
    fn worktrees_with_agents_nested_picks_most_specific() {
        let cwds: HashSet<PathBuf> = [PathBuf::from("/repos/project/worktree-a/src")].into();
        let worktrees = vec![
            PathBuf::from("/repos/project"),
            PathBuf::from("/repos/project/worktree-a"),
        ];
        let matched = worktrees_with_agents(&cwds, &worktrees);
        assert_eq!(matched.len(), 1);
        assert!(matched.contains(Path::new("/repos/project/worktree-a")));
    }

    #[test]
    fn worktrees_with_agents_no_match() {
        let cwds: HashSet<PathBuf> = [PathBuf::from("/completely/different")].into();
        let worktrees = vec![PathBuf::from("/repos/project")];
        let matched = worktrees_with_agents(&cwds, &worktrees);
        assert!(matched.is_empty());
    }

    #[test]
    fn worktrees_with_agents_multiple_agents_multiple_worktrees() {
        let cwds: HashSet<PathBuf> = [
            PathBuf::from("/repos/project-a/src"),
            PathBuf::from("/repos/project-b"),
        ]
        .into();
        let worktrees = vec![
            PathBuf::from("/repos/project-a"),
            PathBuf::from("/repos/project-b"),
            PathBuf::from("/repos/project-c"),
        ];
        let matched = worktrees_with_agents(&cwds, &worktrees);
        assert_eq!(matched.len(), 2);
        assert!(matched.contains(Path::new("/repos/project-a")));
        assert!(matched.contains(Path::new("/repos/project-b")));
    }
}
