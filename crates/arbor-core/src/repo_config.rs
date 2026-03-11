use {
    config::{Config, File},
    serde::Deserialize,
    std::path::{Path, PathBuf},
    thiserror::Error,
};

pub const REPO_CONFIG_FILE_NAME: &str = "arbor.toml";

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RepoConfig {
    pub presets: Vec<RepoPresetConfig>,
    pub processes: Vec<ProcessConfig>,
    pub scripts: RepoScriptsConfig,
    pub notifications: RepoNotificationsConfig,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RepoPresetConfig {
    pub name: String,
    pub icon: String,
    pub command: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProcessConfig {
    pub name: String,
    pub command: String,
    pub working_dir: Option<String>,
    pub auto_start: Option<bool>,
    pub auto_restart: Option<bool>,
    pub restart_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RepoScriptsConfig {
    pub setup: Vec<String>,
    pub teardown: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RepoNotificationsConfig {
    pub desktop: Option<bool>,
    pub events: Vec<String>,
    pub webhook_urls: Vec<String>,
}

#[derive(Debug, Error)]
pub enum RepoConfigError {
    #[error("failed to read repository config `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: config::ConfigError,
    },
}

pub fn repo_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(REPO_CONFIG_FILE_NAME)
}

pub fn read_repo_config(repo_root: &Path) -> Result<Option<RepoConfig>, RepoConfigError> {
    let path = repo_config_path(repo_root);
    if !path.exists() {
        return Ok(None);
    }

    let config = Config::builder()
        .add_source(File::from(path.as_path()).required(false))
        .build()
        .and_then(|settings| settings.try_deserialize::<RepoConfig>())
        .map_err(|source| RepoConfigError::Read {
            path: path.clone(),
            source,
        })?;

    Ok(Some(config))
}

pub fn load_repo_config(repo_root: &Path) -> Option<RepoConfig> {
    read_repo_config(repo_root).ok().flatten()
}

#[cfg(test)]
mod tests {
    use {super::*, std::fs, tempfile::tempdir};

    #[test]
    fn reads_repo_config_with_scripts_and_notifications() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = repo_config_path(dir.path());
        let content = r#"
[[presets]]
name = "Review"
icon = "R"
command = "claude"

[[processes]]
name = "dev"
command = "npm run dev"
auto_start = true

[scripts]
setup = ["cp .env.example .env"]
teardown = ["rm -f .env"]

[notifications]
desktop = true
events = ["agent_finished"]
webhook_urls = ["https://example.com/hook"]
"#;
        if let Err(error) = fs::write(&path, content) {
            panic!("failed to write config: {error}");
        }

        let config = match read_repo_config(dir.path()) {
            Ok(Some(config)) => config,
            Ok(None) => panic!("config should exist"),
            Err(error) => panic!("failed to read config: {error}"),
        };

        assert_eq!(config.presets.len(), 1);
        assert_eq!(config.processes.len(), 1);
        assert_eq!(config.scripts.setup, vec!["cp .env.example .env"]);
        assert_eq!(config.scripts.teardown, vec!["rm -f .env"]);
        assert_eq!(config.notifications.desktop, Some(true));
        assert_eq!(config.notifications.events, vec!["agent_finished"]);
        assert_eq!(config.notifications.webhook_urls, vec![
            "https://example.com/hook"
        ]);
    }
}
