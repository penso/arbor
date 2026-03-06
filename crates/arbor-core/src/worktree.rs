use {
    std::{
        path::{Path, PathBuf},
        process::{Command, Output},
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
    #[error("failed to execute git: {0}")]
    Io(#[from] std::io::Error),
    #[error("git returned non-UTF8 output: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("git command failed: {0}")]
    GitCommandFailed(String),
    #[error("invalid `git worktree list --porcelain` output: {0}")]
    InvalidPorcelain(String),
}

pub fn repo_root(path: &Path) -> Result<PathBuf, WorktreeError> {
    let output = run_git_capture(path, &["rev-parse", "--show-toplevel"])?;
    let stdout = String::from_utf8(output.stdout)?;
    let root = stdout.trim();

    if root.is_empty() {
        return Err(WorktreeError::InvalidPorcelain(
            "empty repository root returned by git".to_owned(),
        ));
    }

    Ok(PathBuf::from(root))
}

pub fn list(path: &Path) -> Result<Vec<Worktree>, WorktreeError> {
    let output = run_git_capture(path, &["worktree", "list", "--porcelain"])?;
    let stdout = String::from_utf8(output.stdout)?;
    parse_porcelain(&stdout)
}

pub fn add(
    repo_path: &Path,
    worktree_path: &Path,
    options: AddWorktreeOptions<'_>,
) -> Result<(), WorktreeError> {
    let mut command = base_git_command(repo_path);
    command.arg("worktree").arg("add");

    if options.force {
        command.arg("--force");
    }

    if options.detach {
        command.arg("--detach");
    }

    if let Some(branch) = options.branch {
        command.arg("-b").arg(branch);
    }

    command.arg(worktree_path);

    run_git_no_output(command)
}

pub fn remove(repo_path: &Path, worktree_path: &Path, force: bool) -> Result<(), WorktreeError> {
    let mut command = base_git_command(repo_path);
    command.arg("worktree").arg("remove");

    if force {
        command.arg("--force");
    }

    command.arg(worktree_path);

    run_git_no_output(command)
}

fn parse_porcelain(output: &str) -> Result<Vec<Worktree>, WorktreeError> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;

    for line in output.lines() {
        if line.is_empty() {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            continue;
        }

        let (field, value) = split_field(line);

        if field == "worktree" {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }

            let path = value.ok_or_else(|| {
                WorktreeError::InvalidPorcelain("missing path after `worktree`".to_owned())
            })?;

            current = Some(Worktree {
                path: PathBuf::from(path),
                ..Worktree::default()
            });
            continue;
        }

        let worktree = current.as_mut().ok_or_else(|| {
            WorktreeError::InvalidPorcelain(format!("field appeared before `worktree`: `{line}`"))
        })?;

        match field {
            "HEAD" => {
                worktree.head = value.map(str::to_owned);
            },
            "branch" => {
                worktree.branch = value.map(str::to_owned);
            },
            "bare" => {
                worktree.is_bare = true;
            },
            "detached" => {
                worktree.is_detached = true;
            },
            "locked" => {
                worktree.lock_reason = value.map(str::to_owned);
            },
            "prunable" => {
                worktree.prune_reason = value.map(str::to_owned);
            },
            _ => {},
        }
    }

    if let Some(worktree) = current.take() {
        worktrees.push(worktree);
    }

    Ok(worktrees)
}

fn split_field(line: &str) -> (&str, Option<&str>) {
    if let Some((field, value)) = line.split_once(' ') {
        return (field, Some(value));
    }

    (line, None)
}

fn run_git_capture(path: &Path, args: &[&str]) -> Result<Output, WorktreeError> {
    let mut command = base_git_command(path);
    command.args(args);

    let output = command.output()?;
    ensure_success(output)
}

fn run_git_no_output(mut command: Command) -> Result<(), WorktreeError> {
    let output = command.output()?;
    let _output = ensure_success(output)?;
    Ok(())
}

fn base_git_command(path: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(path);
    command
}

fn ensure_success(output: Output) -> Result<Output, WorktreeError> {
    if output.status.success() {
        return Ok(output);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let message = if stderr.is_empty() {
        format!("process exited with status {}", output.status)
    } else {
        stderr
    };

    Err(WorktreeError::GitCommandFailed(message))
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use crate::worktree::parse_porcelain;

    #[test]
    fn parses_multiple_worktrees() {
        let input = "worktree /tmp/repo\nHEAD aaaabbbb\nbranch refs/heads/main\n\nworktree /tmp/repo-feature\nHEAD ccccdddd\ndetached\nlocked branch in use\nprunable stale checkout\n\n";

        let parsed = parse_porcelain(input).expect("porcelain should parse");

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].path.to_string_lossy(), "/tmp/repo");
        assert_eq!(parsed[0].branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(parsed[1].path.to_string_lossy(), "/tmp/repo-feature");
        assert!(parsed[1].is_detached);
        assert_eq!(parsed[1].lock_reason.as_deref(), Some("branch in use"));
        assert_eq!(parsed[1].prune_reason.as_deref(), Some("stale checkout"));
    }

    #[test]
    fn rejects_fields_before_worktree_header() {
        let input = "HEAD aaaabbbb\nbranch refs/heads/main\n";
        let error = parse_porcelain(input).expect_err("invalid output should fail");
        assert!(
            error
                .to_string()
                .contains("field appeared before `worktree`")
        );
    }
}
