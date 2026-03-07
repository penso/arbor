use {
    gix::status::{UntrackedFiles, index_worktree::iter::Summary},
    std::{
        fmt, fs,
        path::{Path, PathBuf},
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
    #[error("failed to compute diff in `{path}`: {message}")]
    Diff { path: PathBuf, message: String },
}

pub fn changed_files(repo_path: &Path) -> Result<Vec<ChangedFile>, ChangesError> {
    let repository = open_repository(repo_path)?;

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

        let rela_path_str = String::from_utf8_lossy(item.rela_path().as_ref()).into_owned();
        let path = PathBuf::from(&rela_path_str);
        let kind = summary_to_change_kind(summary);

        let line_summary =
            compute_line_stats_for_file(repo_path, &rela_path_str, kind, &repository);

        files.push(ChangedFile {
            path,
            kind,
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
    let files = changed_files(repo_path)?;
    let mut summary = DiffLineSummary::default();
    for file in &files {
        summary.additions += file.additions;
        summary.deletions += file.deletions;
    }
    Ok(summary)
}

/// Read a blob from HEAD by relative path. Returns `None` if the file
/// doesn't exist at HEAD (e.g. newly added files or initial commit).
fn read_head_blob(repository: &gix::Repository, rela_path: &str) -> Option<Vec<u8>> {
    let spec = format!("HEAD:{rela_path}");
    let object = repository.rev_parse_single(spec.as_str()).ok()?;
    let blob = object.object().ok()?;
    Some(blob.data.to_vec())
}

/// Compute line-level additions/deletions for a single file.
fn compute_line_stats_for_file(
    repo_path: &Path,
    rela_path: &str,
    kind: ChangeKind,
    repository: &gix::Repository,
) -> DiffLineSummary {
    match kind {
        ChangeKind::Added | ChangeKind::IntentToAdd => {
            let abs_path = repo_path.join(rela_path);
            let Ok(contents) = fs::read(&abs_path) else {
                return DiffLineSummary::default();
            };
            DiffLineSummary {
                additions: count_lines(&contents),
                deletions: 0,
            }
        },
        ChangeKind::Removed => {
            let old_bytes = read_head_blob(repository, rela_path).unwrap_or_default();
            DiffLineSummary {
                additions: 0,
                deletions: count_lines(&old_bytes),
            }
        },
        ChangeKind::Modified
        | ChangeKind::TypeChange
        | ChangeKind::Renamed
        | ChangeKind::Copied
        | ChangeKind::Conflict => {
            let old_bytes = read_head_blob(repository, rela_path).unwrap_or_default();
            let abs_path = repo_path.join(rela_path);
            let new_bytes = fs::read(&abs_path).unwrap_or_default();
            diff_line_stats(&old_bytes, &new_bytes)
        },
    }
}

/// Count added/removed lines between two byte slices using imara-diff.
fn diff_line_stats(old: &[u8], new: &[u8]) -> DiffLineSummary {
    use gix_diff::blob::v2::{Algorithm, Diff, InternedInput};

    let input = InternedInput::new(old, new);
    let diff = Diff::compute(Algorithm::Histogram, &input);
    let mut additions = 0usize;
    let mut deletions = 0usize;

    for hunk in diff.hunks() {
        let before_len = (hunk.before.end - hunk.before.start) as usize;
        let after_len = (hunk.after.end - hunk.after.start) as usize;
        deletions += before_len;
        additions += after_len;
    }

    DiffLineSummary {
        additions,
        deletions,
    }
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
