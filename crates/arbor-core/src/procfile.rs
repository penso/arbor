use {
    std::{
        fs,
        path::{Path, PathBuf},
    },
    thiserror::Error,
};

pub const PROCFILE_NAME: &str = "Procfile";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcfileEntry {
    pub name: String,
    pub command: String,
}

#[derive(Debug, Error)]
pub enum ProcfileError {
    #[error("failed to read Procfile `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid Procfile line {line} in `{path}`: {message}")]
    InvalidLine {
        path: PathBuf,
        line: usize,
        message: String,
    },
}

pub fn procfile_path(worktree_root: &Path) -> PathBuf {
    worktree_root.join(PROCFILE_NAME)
}

pub fn read_procfile(worktree_root: &Path) -> Result<Option<Vec<ProcfileEntry>>, ProcfileError> {
    let path = procfile_path(worktree_root);
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path).map_err(|source| ProcfileError::Read {
        path: path.clone(),
        source,
    })?;

    parse_procfile(&content, &path).map(Some)
}

pub fn parse_procfile(content: &str, path: &Path) -> Result<Vec<ProcfileEntry>, ProcfileError> {
    let mut entries = Vec::new();

    for (index, raw_line) in content.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((raw_name, raw_command)) = line.split_once(':') else {
            return Err(ProcfileError::InvalidLine {
                path: path.to_path_buf(),
                line: line_number,
                message: "expected `name: command`".to_owned(),
            });
        };

        let name = raw_name.trim();
        if name.is_empty() {
            return Err(ProcfileError::InvalidLine {
                path: path.to_path_buf(),
                line: line_number,
                message: "process name is empty".to_owned(),
            });
        }
        if !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            return Err(ProcfileError::InvalidLine {
                path: path.to_path_buf(),
                line: line_number,
                message: format!(
                    "process name `{name}` may only contain ASCII letters, digits, `_`, or `-`"
                ),
            });
        }

        let command = raw_command.trim();
        if command.is_empty() {
            return Err(ProcfileError::InvalidLine {
                path: path.to_path_buf(),
                line: line_number,
                message: format!("process `{name}` is missing a command"),
            });
        }

        entries.push(ProcfileEntry {
            name: name.to_owned(),
            command: command.to_owned(),
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_procfile_entries() {
        let entries = parse_procfile(
            "\n# comment\nweb: cargo run\nworker: just jobs\n",
            Path::new("/tmp/Procfile"),
        )
        .unwrap_or_else(|error| panic!("failed to parse Procfile: {error}"));

        assert_eq!(entries, vec![
            ProcfileEntry {
                name: "web".to_owned(),
                command: "cargo run".to_owned(),
            },
            ProcfileEntry {
                name: "worker".to_owned(),
                command: "just jobs".to_owned(),
            },
        ]);
    }

    #[test]
    fn rejects_invalid_procfile_lines() {
        let error = parse_procfile("web cargo run\n", Path::new("/tmp/Procfile"))
            .err()
            .unwrap_or_else(|| panic!("expected parse error"));

        assert!(matches!(error, ProcfileError::InvalidLine { line: 1, .. }));
    }
}
