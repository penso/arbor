use {
    serde::{Deserialize, Serialize},
    std::{
        env, fs,
        path::{Path, PathBuf},
        sync::Arc,
    },
};

const GITHUB_AUTH_STORE_RELATIVE_PATH: &str = ".arbor/github-auth.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GithubAuthState {
    pub access_token: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    pub user_login: Option<String>,
    pub user_avatar_url: Option<String>,
}

pub trait GithubAuthStore: Send + Sync {
    fn load(&self) -> Result<GithubAuthState, String>;
    fn save(&self, state: &GithubAuthState) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub struct JsonGithubAuthStore {
    path: PathBuf,
}

impl JsonGithubAuthStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Default for JsonGithubAuthStore {
    fn default() -> Self {
        Self::new(default_github_auth_store_path())
    }
}

impl GithubAuthStore for JsonGithubAuthStore {
    fn load(&self) -> Result<GithubAuthState, String> {
        if !self.path.exists() {
            return Ok(GithubAuthState::default());
        }

        let raw = fs::read_to_string(&self.path).map_err(|error| {
            format!(
                "failed to read GitHub auth state `{}`: {error}",
                self.path.display()
            )
        })?;

        serde_json::from_str(&raw).map_err(|error| {
            format!(
                "failed to parse GitHub auth state `{}`: {error}",
                self.path.display()
            )
        })
    }

    fn save(&self, state: &GithubAuthState) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create GitHub auth directory `{}`: {error}",
                    parent.display()
                )
            })?;
        }

        let payload = serde_json::to_string_pretty(state).map_err(|error| {
            format!(
                "failed to serialize GitHub auth state for `{}`: {error}",
                self.path.display()
            )
        })?;

        fs::write(&self.path, payload).map_err(|error| {
            format!(
                "failed to write GitHub auth state `{}`: {error}",
                self.path.display()
            )
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&self.path, permissions).map_err(|error| {
                format!(
                    "failed to secure GitHub auth state permissions for `{}`: {error}",
                    self.path.display()
                )
            })?;
        }

        Ok(())
    }
}

pub fn default_github_auth_store() -> Arc<dyn GithubAuthStore> {
    Arc::new(JsonGithubAuthStore::default())
}

fn default_github_auth_store_path() -> PathBuf {
    resolve_home_relative(GITHUB_AUTH_STORE_RELATIVE_PATH)
}

fn resolve_home_relative(relative_path: &str) -> PathBuf {
    home_dir().join(relative_path)
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}
