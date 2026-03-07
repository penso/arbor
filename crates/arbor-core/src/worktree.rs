use {
    std::{
        fs,
        path::{Path, PathBuf},
        time::SystemTime,
    },
    thiserror::Error,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub is_bare: bool,
    pub is_detached: bool,
    pub lock_reason: Option<String>,
    pub prune_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AddWorktreeOptions<'a> {
    pub branch: Option<&'a str>,
    pub detach: bool,
    pub force: bool,
}

#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error("failed to open git repository: {0}")]
    OpenRepository(String),
    #[error("git operation failed: {0}")]
    GitOperationFailed(String),
    #[error("invalid worktree data: {0}")]
    InvalidWorktreeData(String),
}

/// Find the root of the repository (the main worktree path).
///
/// Uses `gix::discover` to find and open the repository, then reads the
/// worktree list to find the primary worktree.
pub fn repo_root(path: &Path) -> Result<PathBuf, WorktreeError> {
    let worktrees = list(path)?;
    if let Some(main) = worktrees.first() {
        return Ok(main.path.clone());
    }

    // Fallback: use gix to discover the repo root.
    let repo = open_gix_repo(path)?;
    match repo.workdir() {
        Some(work_dir) => Ok(work_dir.to_path_buf()),
        None => Err(WorktreeError::InvalidWorktreeData(
            "repository has no working directory".to_owned(),
        )),
    }
}

/// List all worktrees in the repository.
///
/// Opens the repository with `git2` and enumerates worktrees by reading
/// the `.git/worktrees/` directory structure and the main worktree.
pub fn list(path: &Path) -> Result<Vec<Worktree>, WorktreeError> {
    let repo = open_git2_repo(path)?;
    let mut worktrees = Vec::new();

    // The main worktree is always first.
    let main_worktree = build_main_worktree(&repo)?;
    worktrees.push(main_worktree);

    // List linked worktrees via git2.
    let worktree_names = repo.worktrees().map_err(|error| {
        WorktreeError::GitOperationFailed(format!("failed to list worktrees: {error}"))
    })?;

    for name_bytes in &worktree_names {
        let Some(name) = name_bytes else {
            continue;
        };
        if let Some(wt) = build_linked_worktree(&repo, name) {
            worktrees.push(wt);
        }
    }

    Ok(worktrees)
}

/// Create a new worktree.
pub fn add(
    repo_path: &Path,
    worktree_path: &Path,
    options: AddWorktreeOptions<'_>,
) -> Result<(), WorktreeError> {
    let repo = open_git2_repo(repo_path)?;

    if options.detach {
        // For detached worktrees, use git2 to create a worktree from HEAD
        // without a branch reference.
        let head_commit = repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .map_err(|error| {
                WorktreeError::GitOperationFailed(format!("failed to resolve HEAD: {error}"))
            })?;

        let name = worktree_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "worktree".to_owned());

        let mut opts = git2::WorktreeAddOptions::new();
        let reference = repo.find_reference("HEAD").map_err(|error| {
            WorktreeError::GitOperationFailed(format!("failed to find HEAD: {error}"))
        })?;
        opts.reference(Some(&reference));

        repo.worktree(&name, worktree_path, Some(&opts))
            .map_err(|error| {
                WorktreeError::GitOperationFailed(format!("failed to add worktree: {error}"))
            })?;

        // Detach HEAD in the new worktree
        let wt_repo = git2::Repository::open(worktree_path).map_err(|error| {
            WorktreeError::GitOperationFailed(format!("failed to open new worktree: {error}"))
        })?;
        wt_repo
            .set_head_detached(head_commit.id())
            .map_err(|error| {
                WorktreeError::GitOperationFailed(format!("failed to detach HEAD: {error}"))
            })?;
    } else if let Some(branch_name) = options.branch {
        // Create a new branch and worktree.
        let head_commit = repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .map_err(|error| {
                WorktreeError::GitOperationFailed(format!("failed to resolve HEAD: {error}"))
            })?;

        let branch = repo
            .branch(branch_name, &head_commit, options.force)
            .map_err(|error| {
                WorktreeError::GitOperationFailed(format!(
                    "failed to create branch `{branch_name}`: {error}"
                ))
            })?;

        let name = worktree_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| branch_name.to_owned());

        let mut opts = git2::WorktreeAddOptions::new();
        let branch_ref = branch.into_reference();
        opts.reference(Some(&branch_ref));

        repo.worktree(&name, worktree_path, Some(&opts))
            .map_err(|error| {
                WorktreeError::GitOperationFailed(format!("failed to add worktree: {error}"))
            })?;
    } else {
        // Add worktree without a new branch (checkout existing branch).
        let name = worktree_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "worktree".to_owned());

        repo.worktree(&name, worktree_path, None).map_err(|error| {
            WorktreeError::GitOperationFailed(format!("failed to add worktree: {error}"))
        })?;
    }

    Ok(())
}

/// Remove a worktree.
pub fn remove(repo_path: &Path, worktree_path: &Path, force: bool) -> Result<(), WorktreeError> {
    let repo = open_git2_repo(repo_path)?;

    // Find the worktree name by matching its path.
    let worktree_names = repo.worktrees().map_err(|error| {
        WorktreeError::GitOperationFailed(format!("failed to list worktrees: {error}"))
    })?;

    let canonical_target = canonicalize_if_possible(worktree_path.to_path_buf());

    for name_bytes in &worktree_names {
        let Some(name) = name_bytes else {
            continue;
        };

        let Ok(wt) = repo.find_worktree(name) else {
            continue;
        };

        let wt_path = canonicalize_if_possible(wt.path().to_path_buf());
        if wt_path == canonical_target {
            let mut prune_opts = git2::WorktreePruneOptions::new();
            prune_opts.valid(true).working_tree(true);
            if force {
                prune_opts.locked(true);
            }
            wt.prune(Some(&mut prune_opts)).map_err(|error| {
                WorktreeError::GitOperationFailed(format!("failed to remove worktree: {error}"))
            })?;

            // Also remove the worktree directory if it still exists.
            if worktree_path.exists() {
                let _ = fs::remove_dir_all(worktree_path);
            }

            return Ok(());
        }
    }

    Err(WorktreeError::GitOperationFailed(format!(
        "worktree not found: {}",
        worktree_path.display()
    )))
}

/// Returns `true` if the worktree at `path` has commits that haven't been
/// pushed to any remote tracking branch.
pub fn has_unpushed_commits(path: &Path) -> bool {
    let Ok(repo) = gix::open(path) else {
        return false;
    };

    // Try upstream comparison first.
    if let Some(has_unpushed) = check_unpushed_vs_upstream(&repo) {
        return has_unpushed;
    }

    // No upstream — check for commits not reachable from any remote ref.
    check_unpushed_vs_all_remotes(&repo).unwrap_or(false)
}

/// Deletes a local branch.
pub fn delete_branch(repo_path: &Path, branch: &str) -> Result<(), WorktreeError> {
    let repo = open_git2_repo(repo_path)?;
    let mut branch_ref = repo
        .find_branch(branch, git2::BranchType::Local)
        .map_err(|error| {
            WorktreeError::GitOperationFailed(format!("failed to find branch `{branch}`: {error}"))
        })?;
    branch_ref.delete().map_err(|error| {
        WorktreeError::GitOperationFailed(format!("failed to delete branch `{branch}`: {error}"))
    })?;
    Ok(())
}

/// Strips `refs/heads/` prefix from a full git branch ref.
pub fn short_branch(value: &str) -> String {
    value
        .strip_prefix("refs/heads/")
        .unwrap_or(value)
        .to_owned()
}

/// Compares two paths, falling back to canonicalization when they differ textually.
pub fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    let left_canonical = left.canonicalize().ok();
    let right_canonical = right.canonicalize().ok();

    left_canonical
        .zip(right_canonical)
        .is_some_and(|(left, right)| left == right)
}

/// Canonicalizes a path if possible, returning the original on failure.
pub fn canonicalize_if_possible(path: PathBuf) -> PathBuf {
    match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => path,
    }
}

/// Resolves the actual `.git` directory for a worktree path.
///
/// For the main worktree this is simply `<path>/.git`.  For linked worktrees
/// the `.git` entry is a file containing `gitdir: <path>` pointing to a
/// directory inside the main repo's `.git/worktrees/` folder.
pub fn resolve_git_dir(worktree_path: &Path) -> Option<PathBuf> {
    let dot_git = worktree_path.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    if dot_git.is_file() {
        let content = fs::read_to_string(&dot_git).ok()?;
        let gitdir = content.strip_prefix("gitdir: ")?.trim();
        let gitdir_path = PathBuf::from(gitdir);
        let resolved = if gitdir_path.is_relative() {
            worktree_path.join(gitdir_path)
        } else {
            gitdir_path
        };
        if resolved.is_dir() {
            return Some(resolved);
        }
    }
    None
}

/// Returns the most recent modification time (as unix milliseconds) among
/// key git bookkeeping files: `index`, `logs/HEAD`, and `HEAD`.
pub fn last_git_activity_ms(worktree_path: &Path) -> Option<u64> {
    let git_dir = resolve_git_dir(worktree_path)?;
    let candidates = [
        git_dir.join("index"),
        git_dir.join("logs").join("HEAD"),
        git_dir.join("HEAD"),
    ];

    candidates
        .iter()
        .filter_map(|path| fs::metadata(path).ok()?.modified().ok())
        .filter_map(|mtime| {
            mtime
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as u64)
        })
        .max()
}

// --- Private helpers ---

fn open_gix_repo(path: &Path) -> Result<gix::Repository, WorktreeError> {
    match gix::open(path) {
        Ok(repo) => Ok(repo),
        Err(open_err) => gix::discover(path).map_err(|discover_err| {
            WorktreeError::OpenRepository(format!(
                "open error: {open_err}; discover error: {discover_err}"
            ))
        }),
    }
}

fn open_git2_repo(path: &Path) -> Result<git2::Repository, WorktreeError> {
    git2::Repository::discover(path).map_err(|error| {
        WorktreeError::OpenRepository(format!(
            "failed to open repository at `{}`: {error}",
            path.display()
        ))
    })
}

/// Build the main worktree entry from a git2 repository.
fn build_main_worktree(repo: &git2::Repository) -> Result<Worktree, WorktreeError> {
    let work_dir = repo.workdir().ok_or_else(|| {
        WorktreeError::InvalidWorktreeData("repository has no working directory".to_owned())
    })?;

    let head = repo
        .head()
        .ok()
        .and_then(|h| h.target().map(|oid| oid.to_string()));

    let branch = repo
        .head()
        .ok()
        .filter(|h| h.is_branch())
        .and_then(|h| h.name().map(str::to_owned));

    let is_bare = repo.is_bare();

    let is_detached = repo.head_detached().unwrap_or(false);

    Ok(Worktree {
        path: work_dir.to_path_buf(),
        head,
        branch,
        is_bare,
        is_detached,
        lock_reason: None,
        prune_reason: None,
    })
}

/// Build a linked worktree entry by name.
fn build_linked_worktree(repo: &git2::Repository, name: &str) -> Option<Worktree> {
    let wt = repo.find_worktree(name).ok()?;
    let wt_path = wt.path().to_path_buf();

    // Open the worktree as its own repository to read HEAD/branch.
    let wt_repo = git2::Repository::open(&wt_path).ok()?;

    let head = wt_repo
        .head()
        .ok()
        .and_then(|h| h.target().map(|oid| oid.to_string()));

    let branch = wt_repo
        .head()
        .ok()
        .filter(|h| h.is_branch())
        .and_then(|h| h.name().map(str::to_owned));

    let is_detached = wt_repo.head_detached().unwrap_or(false);

    let lock_reason = match wt.is_locked() {
        Ok(git2::WorktreeLockStatus::Locked(reason)) => Some(reason.unwrap_or_default()),
        Ok(git2::WorktreeLockStatus::Unlocked) | Err(_) => None,
    };

    // Check if prunable.
    let is_prunable = wt.validate().is_err();

    let prune_reason = if is_prunable {
        Some("stale checkout".to_owned())
    } else {
        None
    };

    Some(Worktree {
        path: wt_path,
        head,
        branch,
        is_bare: false,
        is_detached,
        lock_reason,
        prune_reason,
    })
}

/// Check if HEAD has commits not reachable from the upstream tracking branch.
fn check_unpushed_vs_upstream(repo: &gix::Repository) -> Option<bool> {
    let head_ref = repo.head_ref().ok()??;
    let head_id = head_ref.id();

    let branch_name = head_ref.name().shorten().to_string();
    let remote_name = repo
        .config_snapshot()
        .string(format!("branch.{branch_name}.remote"))
        .map(|s| s.to_string())?;
    let merge_ref = repo
        .config_snapshot()
        .string(format!("branch.{branch_name}.merge"))
        .map(|s| s.to_string())?;

    // Convert refs/heads/foo -> refs/remotes/origin/foo
    let remote_branch = merge_ref.strip_prefix("refs/heads/")?;
    let upstream_ref_name = format!("refs/remotes/{remote_name}/{remote_branch}");

    let upstream_id = repo.find_reference(&upstream_ref_name).ok()?.id();

    if head_id == upstream_id {
        return Some(false);
    }

    // Check if HEAD is an ancestor of upstream (meaning nothing to push).
    let repo2 = open_git2_from_gix(repo).ok()?;
    let head_oid = git2::Oid::from_bytes(head_id.as_bytes()).ok()?;
    let upstream_oid = git2::Oid::from_bytes(upstream_id.as_bytes()).ok()?;

    // If upstream is an ancestor of head, there are unpushed commits.
    // If head == upstream or head is ancestor of upstream, nothing to push.
    let is_ancestor = repo2
        .graph_descendant_of(head_oid, upstream_oid)
        .unwrap_or(false);
    Some(is_ancestor)
}

/// Check if HEAD has commits not reachable from any remote ref.
fn check_unpushed_vs_all_remotes(repo: &gix::Repository) -> Option<bool> {
    let head_id = repo.head_id().ok()?;
    let repo2 = open_git2_from_gix(repo).ok()?;
    let head_oid = git2::Oid::from_bytes(head_id.as_bytes()).ok()?;

    // Collect all remote tracking ref OIDs.
    let references = repo2.references_glob("refs/remotes/*").ok()?;
    for reference in references.flatten() {
        if let Some(remote_oid) = reference.target() {
            if remote_oid == head_oid {
                return Some(false);
            }
            if repo2
                .graph_descendant_of(remote_oid, head_oid)
                .unwrap_or(false)
            {
                return Some(false);
            }
        }
    }

    Some(true)
}

fn open_git2_from_gix(repo: &gix::Repository) -> Result<git2::Repository, WorktreeError> {
    git2::Repository::open(repo.git_dir()).map_err(|error| {
        WorktreeError::OpenRepository(format!("failed to open with git2: {error}"))
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    #[test]
    fn short_branch_strips_prefix() {
        assert_eq!(super::short_branch("refs/heads/main"), "main");
        assert_eq!(super::short_branch("main"), "main");
        assert_eq!(super::short_branch("refs/heads/feature/foo"), "feature/foo");
    }
}
