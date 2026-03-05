use {
    serde::{Deserialize, Serialize},
    std::{
        env, fs,
        path::{Path, PathBuf},
    },
    thiserror::Error,
};

const DAEMON_SESSION_STORE_RELATIVE_PATH: &str = ".arbor/daemon/sessions.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOrAttachRequest {
    pub session_id: String,
    pub workspace_id: String,
    pub cwd: PathBuf,
    pub shell: String,
    pub cols: u16,
    pub rows: u16,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOrAttachResponse {
    pub is_new_session: bool,
    pub session: DaemonSessionRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteRequest {
    pub session_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResizeRequest {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalRequest {
    pub session_id: String,
    pub signal: TerminalSignal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRequest {
    pub session_id: String,
    pub max_lines: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSignal {
    Interrupt,
    Terminate,
    Kill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalSessionState {
    #[default]
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub session_id: String,
    pub output_tail: String,
    pub exit_code: Option<i32>,
    pub state: TerminalSessionState,
    pub updated_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DaemonSessionRecord {
    pub session_id: String,
    pub workspace_id: String,
    pub cwd: PathBuf,
    pub shell: String,
    pub cols: u16,
    pub rows: u16,
    pub title: Option<String>,
    pub last_command: Option<String>,
    pub output_tail: Option<String>,
    pub exit_code: Option<i32>,
    pub state: Option<TerminalSessionState>,
    pub updated_at_unix_ms: Option<u64>,
}

pub trait TerminalDaemon {
    type Error: std::error::Error + Send + Sync + 'static;

    fn create_or_attach(
        &mut self,
        request: CreateOrAttachRequest,
    ) -> Result<CreateOrAttachResponse, Self::Error>;
    fn write(&mut self, request: WriteRequest) -> Result<(), Self::Error>;
    fn resize(&mut self, request: ResizeRequest) -> Result<(), Self::Error>;
    fn signal(&mut self, request: SignalRequest) -> Result<(), Self::Error>;
    fn detach(&mut self, request: DetachRequest) -> Result<(), Self::Error>;
    fn kill(&mut self, request: KillRequest) -> Result<(), Self::Error>;
    fn snapshot(&self, request: SnapshotRequest) -> Result<Option<TerminalSnapshot>, Self::Error>;
    fn list_sessions(&self) -> Result<Vec<DaemonSessionRecord>, Self::Error>;
}

pub trait DaemonSessionStore {
    fn load(&self) -> Result<Vec<DaemonSessionRecord>, DaemonSessionStoreError>;
    fn save(&self, sessions: &[DaemonSessionRecord]) -> Result<(), DaemonSessionStoreError>;

    fn upsert(&self, session: DaemonSessionRecord) -> Result<(), DaemonSessionStoreError> {
        let mut sessions = self.load()?;
        if let Some(index) = sessions
            .iter()
            .position(|current| current.session_id == session.session_id)
        {
            sessions[index] = session;
        } else {
            sessions.push(session);
        }

        self.save(&sessions)
    }

    fn remove(&self, session_id: &str) -> Result<(), DaemonSessionStoreError> {
        let mut sessions = self.load()?;
        sessions.retain(|session| session.session_id != session_id);
        self.save(&sessions)
    }
}

#[derive(Debug, Error)]
pub enum DaemonSessionStoreError {
    #[error("failed to read daemon session store `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse daemon session store `{path}`: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to create daemon session store directory `{path}`: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize daemon sessions: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to write daemon session store `{path}`: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct JsonDaemonSessionStore {
    path: PathBuf,
}

impl JsonDaemonSessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        match env::var("HOME") {
            Ok(home) => PathBuf::from(home).join(DAEMON_SESSION_STORE_RELATIVE_PATH),
            Err(_) => PathBuf::from(DAEMON_SESSION_STORE_RELATIVE_PATH),
        }
    }

    fn ensure_parent_exists(&self) -> Result<(), DaemonSessionStoreError> {
        let Some(parent) = self.path.parent() else {
            return Ok(());
        };

        fs::create_dir_all(parent).map_err(|source| DaemonSessionStoreError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })
    }
}

impl Default for JsonDaemonSessionStore {
    fn default() -> Self {
        Self::new(Self::default_path())
    }
}

impl DaemonSessionStore for JsonDaemonSessionStore {
    fn load(&self) -> Result<Vec<DaemonSessionRecord>, DaemonSessionStoreError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let raw =
            fs::read_to_string(&self.path).map_err(|source| DaemonSessionStoreError::Read {
                path: self.path.clone(),
                source,
            })?;
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }

        serde_json::from_str(&raw).map_err(|source| DaemonSessionStoreError::Parse {
            path: self.path.clone(),
            source,
        })
    }

    fn save(&self, sessions: &[DaemonSessionRecord]) -> Result<(), DaemonSessionStoreError> {
        self.ensure_parent_exists()?;

        let serialized =
            serde_json::to_string_pretty(sessions).map_err(DaemonSessionStoreError::Serialize)?;

        fs::write(&self.path, format!("{serialized}\n")).map_err(|source| {
            DaemonSessionStoreError::Write {
                path: self.path.clone(),
                source,
            }
        })
    }
}

pub fn default_daemon_session_store() -> JsonDaemonSessionStore {
    JsonDaemonSessionStore::default()
}

pub fn daemon_contract_outline() -> &'static str {
    "create_or_attach -> write -> resize -> signal -> detach -> kill -> snapshot -> list_sessions, with session records persisted by DaemonSessionStore"
}

pub fn normalize_session_store_path(path: &Path) -> PathBuf {
    match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use {
        crate::daemon::{
            DaemonSessionRecord, DaemonSessionStore, JsonDaemonSessionStore,
            normalize_session_store_path,
        },
        std::path::PathBuf,
    };

    #[test]
    fn persists_and_loads_sessions() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("sessions.json");
        let store = JsonDaemonSessionStore::new(path.clone());

        let sessions = vec![DaemonSessionRecord {
            session_id: "session-1".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            cwd: PathBuf::from("/tmp/workspace-1"),
            shell: "/bin/zsh".to_owned(),
            cols: 120,
            rows: 35,
            title: Some("term-1".to_owned()),
            last_command: Some("cargo test".to_owned()),
            output_tail: Some("running tests".to_owned()),
            exit_code: None,
            state: Some(crate::daemon::TerminalSessionState::Running),
            updated_at_unix_ms: Some(1_700_000_000_000),
        }];

        store.save(&sessions)?;
        let loaded = store.load()?;
        assert_eq!(loaded, sessions);
        assert_eq!(normalize_session_store_path(&path), path.canonicalize()?);
        Ok(())
    }

    #[test]
    fn upsert_and_remove_sessions() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("sessions.json");
        let store = JsonDaemonSessionStore::new(path);

        let session = DaemonSessionRecord {
            session_id: "session-1".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            cwd: PathBuf::from("/tmp/workspace-1"),
            shell: "/bin/zsh".to_owned(),
            cols: 100,
            rows: 30,
            title: Some("term-1".to_owned()),
            last_command: Some("git status".to_owned()),
            output_tail: Some("On branch main".to_owned()),
            exit_code: None,
            state: Some(crate::daemon::TerminalSessionState::Running),
            updated_at_unix_ms: Some(1_700_000_000_000),
        };
        store.upsert(session.clone())?;
        store.remove("session-1")?;

        let loaded = store.load()?;
        assert!(loaded.is_empty());
        Ok(())
    }

    #[test]
    fn loads_legacy_records_without_new_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("sessions.json");
        let store = JsonDaemonSessionStore::new(path.clone());
        std::fs::write(
            &path,
            r#"[
  {
    "session_id": "session-1",
    "workspace_id": "workspace-1",
    "cwd": "/tmp/workspace-1",
    "shell": "/bin/zsh",
    "cols": 120,
    "rows": 35
  }
]
"#,
        )?;

        let loaded = store.load()?;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].session_id, "session-1");
        assert!(loaded[0].title.is_none());
        assert!(loaded[0].state.is_none());
        Ok(())
    }
}
