use std::{collections::HashSet, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Working,
    Waiting,
}

const AGENT_PROCESS_NAMES: &[&str] = &["claude", "codex", "opencode"];

/// Detect working directories of running AI tool processes.
///
/// Uses the `sysinfo` crate to enumerate processes in-process,
/// avoiding subprocess calls to `pgrep` and `lsof`.
pub fn detect_agent_cwds() -> HashSet<PathBuf> {
    use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};

    let system = System::new_with_specifics(
        RefreshKind::nothing()
            .with_processes(ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always)),
    );

    let mut cwds = HashSet::new();
    for process in system.processes().values() {
        let name = process.name().to_string_lossy();
        if AGENT_PROCESS_NAMES.iter().any(|agent| *agent == name)
            && let Some(cwd) = process.cwd()
            && cwd.is_absolute()
        {
            cwds.insert(cwd.to_path_buf());
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
