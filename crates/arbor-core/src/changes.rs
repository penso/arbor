use {
    gix::status::{UntrackedFiles, index_worktree::iter::Summary},
    std::{
        collections::HashMap,
        fmt, fs,
        path::{Path, PathBuf},
        process::Command,
    },
    thiserror::Error,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
    Renamed,
    Copied,
    TypeChange,
    Conflict,
    IntentToAdd,
}

impl fmt::Display for ChangeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            ChangeKind::Added => "added",
            ChangeKind::Modified => "modified",
            ChangeKind::Removed => "removed",
            ChangeKind::Renamed => "renamed",
            ChangeKind::Copied => "copied",
            ChangeKind::TypeChange => "type-change",
            ChangeKind::Conflict => "conflict",
            ChangeKind::IntentToAdd => "intent-to-add",
        };

        f.write_str(label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub kind: ChangeKind,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffLineSummary {
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Error)]
pub enum ChangesError {
    #[error("failed to open git repository at `{path}`: {message}")]
    OpenRepository { path: PathBuf, message: String },
    #[error("failed to read git status in `{path}`: {message}")]
    Status { path: PathBuf, message: String },
    #[error("failed to run git command in `{path}`: {message}")]
    GitCommand { path: PathBuf, message: String },
}

pub fn changed_files(repo_path: &Path) -> Result<Vec<ChangedFile>, ChangesError> {
    let repository = open_repository(repo_path)?;
    let summary_by_path = diff_line_summary_by_path(repo_path)?;

    let status_iter = repository
        .status(gix::progress::Discard)
        .map_err(|error| ChangesError::Status {
            path: repo_path.to_path_buf(),
            message: error.to_string(),
        })?
        .untracked_files(UntrackedFiles::Files)
        .into_index_worktree_iter(Vec::<gix::bstr::BString>::new())
        .map_err(|error| ChangesError::Status {
            path: repo_path.to_path_buf(),
            message: error.to_string(),
        })?;

    let mut files = Vec::new();

    for item_result in status_iter {
        let item = item_result.map_err(|error| ChangesError::Status {
            path: repo_path.to_path_buf(),
            message: error.to_string(),
        })?;

        let Some(summary) = item.summary() else {
            continue;
        };

        let path = PathBuf::from(String::from_utf8_lossy(item.rela_path().as_ref()).into_owned());
        let line_summary = summary_by_path.get(&path).copied().unwrap_or_default();

        files.push(ChangedFile {
            path,
            kind: summary_to_change_kind(summary),
            additions: line_summary.additions,
            deletions: line_summary.deletions,
        });
    }

    files.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    files.dedup_by(|left, right| left.path == right.path && left.kind == right.kind);

    Ok(files)
}

pub fn diff_line_summary(repo_path: &Path) -> Result<DiffLineSummary, ChangesError> {
    let mut summary = DiffLineSummary::default();
    for line_summary in diff_line_summary_by_path(repo_path)?.values() {
        summary.additions += line_summary.additions;
        summary.deletions += line_summary.deletions;
    }

    Ok(summary)
}

fn diff_line_summary_by_path(
    repo_path: &Path,
) -> Result<HashMap<PathBuf, DiffLineSummary>, ChangesError> {
    let mut summary_by_path = parse_numstat_output_by_path(&run_git_command(repo_path, &[
        "diff",
        "--numstat",
        "HEAD",
        "--",
    ])?);

    let untracked_output = run_git_command_bytes(repo_path, &[
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
    ])?;

    for relative_path in untracked_output.split(|byte| *byte == b'\0') {
        if relative_path.is_empty() {
            continue;
        }

        let relative = String::from_utf8_lossy(relative_path).into_owned();
        let absolute_path = repo_path.join(&relative);
        let Ok(contents) = fs::read(&absolute_path) else {
            continue;
        };

        let entry = summary_by_path.entry(PathBuf::from(relative)).or_default();
        entry.additions += count_lines(&contents);
    }

    Ok(summary_by_path)
}

fn open_repository(path: &Path) -> Result<gix::Repository, ChangesError> {
    match gix::open(path.to_path_buf()) {
        Ok(repo) => Ok(repo),
        Err(open_error) => {
            let fallback =
                gix::discover(path).map_err(|discover_error| ChangesError::OpenRepository {
                    path: path.to_path_buf(),
                    message: format!("open error: {open_error}; discover error: {discover_error}",),
                })?;

            Ok(fallback)
        },
    }
}

fn summary_to_change_kind(summary: Summary) -> ChangeKind {
    match summary {
        Summary::Added => ChangeKind::Added,
        Summary::Modified => ChangeKind::Modified,
        Summary::Removed => ChangeKind::Removed,
        Summary::Renamed => ChangeKind::Renamed,
        Summary::Copied => ChangeKind::Copied,
        Summary::TypeChange => ChangeKind::TypeChange,
        Summary::Conflict => ChangeKind::Conflict,
        Summary::IntentToAdd => ChangeKind::IntentToAdd,
    }
}

fn run_git_command(repo_path: &Path, args: &[&str]) -> Result<String, ChangesError> {
    let output = run_git_command_bytes(repo_path, args)?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

fn run_git_command_bytes(repo_path: &Path, args: &[&str]) -> Result<Vec<u8>, ChangesError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()
        .map_err(|error| ChangesError::GitCommand {
            path: repo_path.to_path_buf(),
            message: format!("failed to execute git {}: {error}", args.join(" ")),
        })?;

    if output.status.success() {
        return Ok(output.stdout);
    }

    Err(ChangesError::GitCommand {
        path: repo_path.to_path_buf(),
        message: format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr),
        ),
    })
}

fn parse_numstat_output_by_path(output: &str) -> HashMap<PathBuf, DiffLineSummary> {
    let mut summary_by_path = HashMap::new();

    for line in output.lines() {
        let mut columns = line.split('\t');
        let Some(added_column) = columns.next() else {
            continue;
        };
        let Some(removed_column) = columns.next() else {
            continue;
        };
        let Some(path_column) = columns.next() else {
            continue;
        };

        let additions = added_column.parse::<usize>().unwrap_or(0);
        let deletions = removed_column.parse::<usize>().unwrap_or(0);
        if additions == 0 && deletions == 0 {
            continue;
        }

        let path = PathBuf::from(path_column);
        let entry = summary_by_path
            .entry(path)
            .or_insert_with(DiffLineSummary::default);
        entry.additions += additions;
        entry.deletions += deletions;
    }

    summary_by_path
}

fn count_lines(contents: &[u8]) -> usize {
    if contents.is_empty() {
        return 0;
    }

    let newline_count = contents.iter().filter(|byte| **byte == b'\n').count();
    let trailing_newline = contents.last().is_some_and(|byte| *byte == b'\n');
    if trailing_newline {
        newline_count
    } else {
        newline_count + 1
    }
}
