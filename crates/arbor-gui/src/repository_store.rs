use {
    crate::checkout::CheckoutKind,
    arbor_core::worktree,
    serde::{Deserialize, Serialize},
    std::{
        collections::{HashMap, HashSet},
        env, fs,
        path::{Path, PathBuf},
        sync::Arc,
    },
};

const REPOSITORY_STORE_RELATIVE_PATH: &str = ".arbor/repositories.json";

pub trait RepositoryStore: Send + Sync {
    fn load_entries(&self) -> Result<Vec<StoredRepositoryEntry>, String>;
    fn save_entries(&self, entries: &[StoredRepositoryEntry]) -> Result<(), String>;
    fn has_store_file(&self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryCheckoutRoot {
    pub path: PathBuf,
    pub kind: CheckoutKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredRepositoryEntry {
    pub root: PathBuf,
    #[serde(default)]
    pub group_key: Option<String>,
    #[serde(default)]
    pub kind: CheckoutKind,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RepositoryStorePayload {
    Legacy(Vec<String>),
    Entries(Vec<StoredRepositoryEntry>),
}

pub struct JsonRepositoryStore {
    path: PathBuf,
}

impl JsonRepositoryStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl RepositoryStore for JsonRepositoryStore {
    fn load_entries(&self) -> Result<Vec<StoredRepositoryEntry>, String> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let raw = fs::read_to_string(&self.path).map_err(|error| {
            format!(
                "failed to read repository store `{}`: {error}",
                self.path.display()
            )
        })?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }

        let payload: RepositoryStorePayload = serde_json::from_str(&raw).map_err(|error| {
            format!(
                "failed to parse repository store `{}`: {error}",
                self.path.display()
            )
        })?;

        Ok(match payload {
            RepositoryStorePayload::Legacy(roots) => roots
                .into_iter()
                .filter(|root| !root.trim().is_empty())
                .map(|root| StoredRepositoryEntry {
                    root: PathBuf::from(root),
                    group_key: None,
                    kind: CheckoutKind::LinkedWorktree,
                })
                .collect(),
            RepositoryStorePayload::Entries(entries) => entries
                .into_iter()
                .filter(|entry| !entry.root.as_os_str().is_empty())
                .collect(),
        })
    }

    fn save_entries(&self, entries: &[StoredRepositoryEntry]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create repository store directory `{}`: {error}",
                    parent.display()
                )
            })?;
        }

        let content = serde_json::to_string_pretty(entries).map_err(|error| {
            format!(
                "failed to serialize repository store `{}`: {error}",
                self.path.display()
            )
        })?;

        fs::write(&self.path, format!("{content}\n")).map_err(|error| {
            format!(
                "failed to write repository store `{}`: {error}",
                self.path.display()
            )
        })
    }

    fn has_store_file(&self) -> bool {
        self.path.exists()
    }
}

pub fn default_repository_store() -> Arc<dyn RepositoryStore> {
    Arc::new(JsonRepositoryStore::new(default_repository_store_path()))
}

fn default_repository_store_path() -> PathBuf {
    match env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(REPOSITORY_STORE_RELATIVE_PATH),
        Err(_) => PathBuf::from(REPOSITORY_STORE_RELATIVE_PATH),
    }
}

pub fn resolve_repositories_from_entries(
    entries: Vec<StoredRepositoryEntry>,
) -> Vec<crate::RepositorySummary> {
    #[derive(Debug)]
    struct RepositoryGroupBuilder {
        representative_root: PathBuf,
        representative_kind: CheckoutKind,
        checkout_roots: Vec<RepositoryCheckoutRoot>,
        seen_roots: HashSet<PathBuf>,
    }

    let mut repositories: Vec<RepositoryGroupBuilder> = Vec::new();
    let mut group_order = Vec::new();
    let mut repository_index_by_group_key = HashMap::<String, usize>::new();

    for entry in entries {
        let Some(normalized_root) = normalize_checkout_root(entry.root, entry.kind) else {
            continue;
        };
        let group_key = normalize_group_key(entry.group_key, &normalized_root, entry.kind);
        let next_root = RepositoryCheckoutRoot {
            path: normalized_root.clone(),
            kind: entry.kind,
        };

        if let Some(index) = repository_index_by_group_key.get(&group_key).copied() {
            let group = &mut repositories[index];
            if group.seen_roots.insert(normalized_root.clone()) {
                if should_prefer_checkout_root(
                    group.representative_kind,
                    group.representative_root.as_path(),
                    next_root.kind,
                    normalized_root.as_path(),
                ) {
                    group.representative_root = normalized_root;
                    group.representative_kind = next_root.kind;
                }
                group.checkout_roots.push(next_root);
            }
            continue;
        }

        repository_index_by_group_key.insert(group_key.clone(), repositories.len());
        group_order.push(group_key);
        repositories.push(RepositoryGroupBuilder {
            representative_root: normalized_root.clone(),
            representative_kind: next_root.kind,
            checkout_roots: vec![next_root],
            seen_roots: HashSet::from([normalized_root]),
        });
    }

    group_order
        .into_iter()
        .map(|group_key| {
            let index = repository_index_by_group_key[&group_key];
            let group = &repositories[index];
            crate::RepositorySummary::from_checkout_roots(
                group.representative_root.clone(),
                group_key,
                group.checkout_roots.clone(),
            )
        })
        .collect()
}

pub fn repository_entries_from_summaries(
    repositories: &[crate::RepositorySummary],
) -> Vec<StoredRepositoryEntry> {
    repositories
        .iter()
        .flat_map(|repository| {
            repository
                .checkout_roots
                .iter()
                .map(|checkout_root| StoredRepositoryEntry {
                    root: checkout_root.path.clone(),
                    group_key: Some(repository.group_key.clone()),
                    kind: checkout_root.kind,
                })
        })
        .collect()
}

pub fn default_group_key_for_root(root: &Path) -> String {
    canonicalize_if_possible(root.to_path_buf())
        .display()
        .to_string()
}

fn normalize_checkout_root(root: PathBuf, kind: CheckoutKind) -> Option<PathBuf> {
    if root.as_os_str().is_empty() {
        return None;
    }

    let normalized = canonicalize_if_possible(root);
    match kind {
        CheckoutKind::LinkedWorktree => worktree::repo_root(&normalized)
            .ok()
            .map(canonicalize_if_possible),
        CheckoutKind::DiscreteClone => Some(normalized),
    }
}

fn normalize_group_key(
    group_key: Option<String>,
    checkout_root: &Path,
    kind: CheckoutKind,
) -> String {
    match kind {
        CheckoutKind::LinkedWorktree => default_group_key_for_root(checkout_root),
        CheckoutKind::DiscreteClone => group_key
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| default_group_key_for_root(checkout_root)),
    }
}

fn should_prefer_checkout_root(
    current_kind: CheckoutKind,
    current_root: &Path,
    candidate_kind: CheckoutKind,
    candidate_root: &Path,
) -> bool {
    match (current_kind, candidate_kind) {
        (CheckoutKind::DiscreteClone, CheckoutKind::LinkedWorktree) => true,
        (CheckoutKind::LinkedWorktree, CheckoutKind::DiscreteClone) => false,
        _ => current_root > candidate_root,
    }
}

fn canonicalize_if_possible(path: PathBuf) -> PathBuf {
    worktree::canonicalize_if_possible(path)
}

#[cfg(test)]
mod tests {
    use {
        super::{
            JsonRepositoryStore, RepositoryStore, StoredRepositoryEntry,
            default_group_key_for_root, resolve_repositories_from_entries,
        },
        crate::checkout::CheckoutKind,
        std::{
            path::PathBuf,
            time::{SystemTime, UNIX_EPOCH},
        },
    };

    fn temp_store_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "arbor-repository-store-tests-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    #[test]
    fn parses_legacy_repository_root_arrays() -> Result<(), Box<dyn std::error::Error>> {
        let path = temp_store_path("legacy-repositories.json");
        std::fs::write(
            &path,
            r#"[
  "/tmp/repo-a",
  "/tmp/repo-b"
]
"#,
        )?;

        let store = JsonRepositoryStore::new(path);
        let entries = store.load_entries()?;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].root, PathBuf::from("/tmp/repo-a"));
        assert_eq!(entries[0].kind, CheckoutKind::LinkedWorktree);
        Ok(())
    }

    #[test]
    fn parses_structured_repository_entries() -> Result<(), Box<dyn std::error::Error>> {
        let path = temp_store_path("structured-repositories.json");
        std::fs::write(
            &path,
            r#"[
  {
    "root": "/tmp/repo-a",
    "group_key": "/tmp/repo-a",
    "kind": "linked_worktree"
  },
  {
    "root": "/tmp/repo-a-clone",
    "group_key": "/tmp/repo-a",
    "kind": "discrete_clone"
  }
]
"#,
        )?;

        let store = JsonRepositoryStore::new(path);
        let entries = store.load_entries()?;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].kind, CheckoutKind::DiscreteClone);
        assert_eq!(entries[1].group_key.as_deref(), Some("/tmp/repo-a"));
        Ok(())
    }

    #[test]
    fn groups_discrete_clones_under_shared_group_key() -> Result<(), Box<dyn std::error::Error>> {
        let repo_root = temp_store_path("repo-root");
        std::fs::create_dir_all(&repo_root)?;
        let _ = git2::Repository::init(&repo_root)?;
        let clone_root = temp_store_path("repo-clone");
        std::fs::create_dir_all(&clone_root)?;
        let _ = git2::Repository::init(&clone_root)?;

        let repositories = resolve_repositories_from_entries(vec![
            StoredRepositoryEntry {
                root: repo_root.clone(),
                group_key: None,
                kind: CheckoutKind::LinkedWorktree,
            },
            StoredRepositoryEntry {
                root: clone_root,
                group_key: Some(default_group_key_for_root(&repo_root)),
                kind: CheckoutKind::DiscreteClone,
            },
        ]);
        let expected_root = arbor_core::worktree::canonicalize_if_possible(repo_root);

        assert_eq!(repositories.len(), 1);
        assert_eq!(repositories[0].checkout_roots.len(), 2);
        assert_eq!(repositories[0].root, expected_root);
        Ok(())
    }

    #[test]
    fn ignores_stale_group_key_for_linked_worktree_entries()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo_root = temp_store_path("repo-root-linked");
        std::fs::create_dir_all(&repo_root)?;
        let repo = git2::Repository::init(&repo_root)?;
        std::fs::write(repo_root.join("README.md"), "hello\n")?;

        let mut index = repo.index()?;
        index.add_path(std::path::Path::new("README.md"))?;
        index.write()?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let signature = git2::Signature::now("Arbor Test", "arbor@example.com")?;
        repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])?;
        drop(tree);

        let worktree_root = temp_store_path("repo-worktree-linked");
        arbor_core::worktree::add(
            &repo_root,
            &worktree_root,
            arbor_core::worktree::AddWorktreeOptions {
                branch: Some("feature/store-group-key"),
                ..Default::default()
            },
        )?;

        let repositories = resolve_repositories_from_entries(vec![StoredRepositoryEntry {
            root: worktree_root.clone(),
            group_key: Some(worktree_root.display().to_string()),
            kind: CheckoutKind::LinkedWorktree,
        }]);
        let expected_root = arbor_core::worktree::canonicalize_if_possible(repo_root.clone());

        assert_eq!(repositories.len(), 1);
        assert_eq!(repositories[0].root, expected_root);
        assert_eq!(
            repositories[0].group_key,
            default_group_key_for_root(&repo_root)
        );

        let _ = std::fs::remove_dir_all(&worktree_root);
        let _ = std::fs::remove_dir_all(&repo_root);
        Ok(())
    }
}
