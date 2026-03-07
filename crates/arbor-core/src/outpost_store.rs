use {
    crate::outpost::OutpostRecord,
    std::{
        env, fs,
        path::{Path, PathBuf},
    },
    thiserror::Error,
};

const OUTPOST_STORE_RELATIVE_PATH: &str = ".arbor/outposts.json";

pub trait OutpostStore {
    fn load(&self) -> Result<Vec<OutpostRecord>, OutpostStoreError>;
    fn save(&self, outposts: &[OutpostRecord]) -> Result<(), OutpostStoreError>;

    fn upsert(&self, outpost: OutpostRecord) -> Result<(), OutpostStoreError> {
        let mut outposts = self.load()?;
        if let Some(index) = outposts.iter().position(|current| current.id == outpost.id) {
            outposts[index] = outpost;
        } else {
            outposts.push(outpost);
        }

        self.save(&outposts)
    }

    fn remove(&self, outpost_id: &str) -> Result<(), OutpostStoreError> {
        let mut outposts = self.load()?;
        outposts.retain(|outpost| outpost.id != outpost_id);
        self.save(&outposts)
    }

    fn outposts_for_repo(
        &self,
        local_repo_root: &str,
    ) -> Result<Vec<OutpostRecord>, OutpostStoreError> {
        let outposts = self.load()?;
        Ok(outposts
            .into_iter()
            .filter(|outpost| outpost.local_repo_root == local_repo_root)
            .collect())
    }
}

#[derive(Debug, Error)]
pub enum OutpostStoreError {
    #[error("failed to read outpost store `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse outpost store `{path}`: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to create outpost store directory `{path}`: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize outposts: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to write outpost store `{path}`: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct JsonOutpostStore {
    path: PathBuf,
}

impl JsonOutpostStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        match env::var("HOME") {
            Ok(home) => PathBuf::from(home).join(OUTPOST_STORE_RELATIVE_PATH),
            Err(_) => PathBuf::from(OUTPOST_STORE_RELATIVE_PATH),
        }
    }

    fn ensure_parent_exists(&self) -> Result<(), OutpostStoreError> {
        let Some(parent) = self.path.parent() else {
            return Ok(());
        };

        fs::create_dir_all(parent).map_err(|source| OutpostStoreError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })
    }
}

impl Default for JsonOutpostStore {
    fn default() -> Self {
        Self::new(Self::default_path())
    }
}

impl OutpostStore for JsonOutpostStore {
    fn load(&self) -> Result<Vec<OutpostRecord>, OutpostStoreError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let raw = fs::read_to_string(&self.path).map_err(|source| OutpostStoreError::Read {
            path: self.path.clone(),
            source,
        })?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }

        serde_json::from_str(&raw).map_err(|source| OutpostStoreError::Parse {
            path: self.path.clone(),
            source,
        })
    }

    fn save(&self, outposts: &[OutpostRecord]) -> Result<(), OutpostStoreError> {
        self.ensure_parent_exists()?;

        let serialized =
            serde_json::to_string_pretty(outposts).map_err(OutpostStoreError::Serialize)?;

        fs::write(&self.path, format!("{serialized}\n")).map_err(|source| {
            OutpostStoreError::Write {
                path: self.path.clone(),
                source,
            }
        })
    }
}

pub fn default_outpost_store() -> JsonOutpostStore {
    JsonOutpostStore::default()
}

pub fn normalize_outpost_store_path(path: &Path) -> PathBuf {
    match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        outpost::OutpostRecord,
        outpost_store::{JsonOutpostStore, OutpostStore, normalize_outpost_store_path},
    };

    fn sample_outpost(id: &str, repo_root: &str) -> OutpostRecord {
        OutpostRecord {
            id: id.to_owned(),
            host_name: "build-server".to_owned(),
            local_repo_root: repo_root.to_owned(),
            remote_path: "~/arbor-outposts/my-project".to_owned(),
            clone_url: "git@github.com:user/repo.git".to_owned(),
            branch: "main".to_owned(),
            label: "my-outpost".to_owned(),
            has_remote_daemon: false,
        }
    }

    #[test]
    fn persists_and_loads_outposts() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("outposts.json");
        let store = JsonOutpostStore::new(path.clone());

        let outposts = vec![sample_outpost("outpost-1", "/home/dev/my-project")];

        store.save(&outposts)?;
        let loaded = store.load()?;
        assert_eq!(loaded, outposts);
        assert_eq!(normalize_outpost_store_path(&path), path.canonicalize()?);
        Ok(())
    }

    #[test]
    fn upsert_and_remove_outposts() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("outposts.json");
        let store = JsonOutpostStore::new(path);

        let outpost = sample_outpost("outpost-1", "/home/dev/my-project");
        store.upsert(outpost.clone())?;

        let loaded = store.load()?;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], outpost);

        store.remove("outpost-1")?;
        let loaded = store.load()?;
        assert!(loaded.is_empty());
        Ok(())
    }

    #[test]
    fn upsert_updates_existing_outpost() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("outposts.json");
        let store = JsonOutpostStore::new(path);

        let outpost = sample_outpost("outpost-1", "/home/dev/my-project");
        store.upsert(outpost)?;

        let mut updated = sample_outpost("outpost-1", "/home/dev/my-project");
        updated.branch = "feature-branch".to_owned();
        updated.label = "updated-label".to_owned();
        store.upsert(updated.clone())?;

        let loaded = store.load()?;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].branch, "feature-branch");
        assert_eq!(loaded[0].label, "updated-label");
        Ok(())
    }

    #[test]
    fn outposts_for_repo_filters_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("outposts.json");
        let store = JsonOutpostStore::new(path);

        store.upsert(sample_outpost("outpost-1", "/home/dev/project-a"))?;
        store.upsert(sample_outpost("outpost-2", "/home/dev/project-b"))?;
        store.upsert(sample_outpost("outpost-3", "/home/dev/project-a"))?;

        let filtered = store.outposts_for_repo("/home/dev/project-a")?;
        assert_eq!(filtered.len(), 2);
        assert!(
            filtered
                .iter()
                .all(|o| o.local_repo_root == "/home/dev/project-a")
        );
        Ok(())
    }

    #[test]
    fn loads_empty_when_file_missing() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("nonexistent.json");
        let store = JsonOutpostStore::new(path);
        let loaded = store.load()?;
        assert!(loaded.is_empty());
        Ok(())
    }

    #[test]
    fn loads_empty_when_file_is_blank() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("outposts.json");
        std::fs::write(&path, "  \n  ")?;
        let store = JsonOutpostStore::new(path);
        let loaded = store.load()?;
        assert!(loaded.is_empty());
        Ok(())
    }
}
