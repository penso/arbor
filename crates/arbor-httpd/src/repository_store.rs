use {
    arbor_core::worktree,
    serde::Deserialize,
    std::{
        collections::HashSet,
        env, fs,
        path::{Path, PathBuf},
    },
};

const REPOSITORY_STORE_RELATIVE_PATH: &str = ".arbor/repositories.json";

#[derive(Debug, Deserialize)]
#[serde(transparent)]
struct RepositoryRoots(Vec<String>);

pub fn default_repository_store_path() -> PathBuf {
    match env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(REPOSITORY_STORE_RELATIVE_PATH),
        Err(_) => PathBuf::from(REPOSITORY_STORE_RELATIVE_PATH),
    }
}

pub fn load_repository_roots(path: &Path) -> Result<Vec<PathBuf>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(path).map_err(|error| {
        format!(
            "failed to read repository store `{}`: {error}",
            path.display()
        )
    })?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    let parsed: RepositoryRoots = serde_json::from_str(&raw).map_err(|error| {
        format!(
            "failed to parse repository store `{}` as JSON array: {error}",
            path.display()
        )
    })?;

    Ok(parsed
        .0
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .collect())
}

pub fn resolve_repository_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut repositories = Vec::new();
    let mut seen = HashSet::new();

    for root in roots {
        let normalized = canonicalize_if_possible(root);
        let repository_root = match worktree::repo_root(&normalized) {
            Ok(path) => canonicalize_if_possible(path),
            Err(_) => continue,
        };

        if seen.insert(repository_root.clone()) {
            repositories.push(repository_root);
        }
    }

    repositories
}

fn canonicalize_if_possible(path: PathBuf) -> PathBuf {
    worktree::canonicalize_if_possible(path)
}

#[cfg(test)]
mod tests {
    use crate::repository_store::load_repository_roots;

    #[test]
    fn parses_repository_roots_as_json_array() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("repositories.json");
        std::fs::write(
            &path,
            r#"[
  "/tmp/repo-a",
  "/tmp/repo-b"
]
"#,
        )?;

        let roots = load_repository_roots(&path)?;
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].to_string_lossy(), "/tmp/repo-a");
        Ok(())
    }
}
