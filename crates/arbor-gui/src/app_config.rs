use {
    crate::StoreError,
    arbor_core::repo_config,
    config::{Config, File},
    serde::Deserialize,
    std::{
        env, fs,
        path::{Path, PathBuf},
        sync::Arc,
        time::SystemTime,
    },
    toml_edit::DocumentMut,
};

const CONFIG_RELATIVE_PATH: &str = ".config/arbor/config.toml";
const DEFAULT_CONFIG_CONTENT: &str = r#"# Arbor configuration
# embedded_terminal_engine = "ghostty-vt-experimental" # default when built with ghostty support, or set to "alacritty"
# embedded_shell = "/usr/bin/fish"  # shell for embedded terminal (defaults to $SHELL, then /bin/zsh)
# terminal_scrollback_lines = 10000 # lines kept in terminal history (min 100, max 100000)
# theme = "one-dark"            # one-dark | ayu-dark | gruvbox-dark | dracula | solarized-light | everforest-dark | catppuccin | catppuccin-latte | ethereal | flexoki-light | hackerman | kanagawa | matte-black | miasma | nord | osaka-jade | ristretto | rose-pine | tokyo-night | vantablack | white | atom-one-light | github-light-default | github-light-high-contrast | github-light-colorblind | github-light | github-dark-default | github-dark-high-contrast | github-dark-colorblind | github-dark-dimmed | github-dark | retrobox-classic | tokyonight-day | tokyonight-classic | zellner
# daemon_url = "http://127.0.0.1:8787" # arbor-httpd base URL
# notifications = true
#
# [[agent_presets]]
# key = "codex"     # codex | claude | pi | opencode | copilot
# command = "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox -c model_reasoning_summary=\"detailed\" -c model_supports_reasoning_summaries=true"
#
# [[agent_presets]]
# key = "claude"
# command = "claude --dangerously-skip-permissions"
#
# [[agent_presets]]
# key = "pi"
# command = "pi"
#
# [[agent_presets]]
# key = "opencode"
# command = "opencode"
#
# [[agent_presets]]
# key = "copilot"
# command = "copilot --allow-all"

# OpenAI-compatible providers (Ollama, LM Studio, OpenRouter, etc.)
# Arbor probes /v1/models at startup when fetch_models = true.
#
# [[providers]]
# name = "Ollama"
# kind = "openai-compatible"
# base_url = "http://localhost:11434/v1"
# fetch_models = true
# # models = [
# #   { id = "llama3.1:70b", label = "Llama 3.1 70B" },
# # ]
#
# [[providers]]
# name = "LM Studio"
# kind = "openai-compatible"
# base_url = "http://localhost:1234/v1"
# fetch_models = true
#
# [[providers]]
# name = "OpenRouter"
# kind = "openai-compatible"
# base_url = "https://openrouter.ai/api/v1"
# api_key_env = "OPENROUTER_API_KEY"
# fetch_models = true
#
# [[providers]]
# name = "OpenAI"
# kind = "openai-compatible"
# base_url = "https://api.openai.com/v1"
# api_key_env = "OPENAI_API_KEY"
# models = [
#   { id = "gpt-4o", label = "GPT-4o" },
#   { id = "gpt-4o-mini", label = "GPT-4o Mini" },
# ]

# [daemon]
# auth_token = "your-secret-token"  # required for remote access
# bind = "all-interfaces"           # all-interfaces | localhost
# tls = true                        # HTTPS with self-signed certs (default: true)

# [[remote_hosts]]
# name = "build-server"
# hostname = "build.example.com"
# user = "dev"
# port = 22
# identity_file = "~/.ssh/id_ed25519"
# remote_base_path = "~/arbor-outposts"
# daemon_port = 8787
# mosh = true                     # use mosh for interactive shells
# mosh_server_path = "/usr/bin/mosh-server"  # optional custom path
"#;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ArborConfig {
    pub terminal_backend: Option<String>,
    pub embedded_terminal_engine: Option<String>,
    pub embedded_shell: Option<String>,
    pub terminal_scrollback_lines: Option<usize>,
    pub theme: Option<String>,
    pub daemon_url: Option<String>,
    pub notifications: Option<bool>,
    pub preferred_editor: Option<String>,
    pub agent_presets: Vec<AgentPresetConfig>,
    pub providers: Vec<ProviderConfig>,
    pub remote_hosts: Vec<RemoteHostConfig>,
    pub daemon: Option<DaemonConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub auth_token: Option<String>,
    pub bind: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AgentPresetConfig {
    pub key: String,
    pub command: String,
}

/// An OpenAI-compatible provider configured in `config.toml`.
///
/// ```toml
/// [[providers]]
/// name = "Ollama"
/// kind = "openai-compatible"
/// base_url = "http://localhost:11434/v1"
/// # api_key = "optional"
/// # api_key_env = "OLLAMA_API_KEY"  # read from env var
/// # fetch_models = true             # probe /v1/models at startup
/// # models = [
/// #   { id = "llama3.1:70b", label = "Llama 3.1 70B" },
/// # ]
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Display name (e.g. "Ollama", "OpenRouter").
    pub name: String,
    /// Provider kind. Currently only `"openai-compatible"` is supported.
    pub kind: String,
    /// API base URL (e.g. `http://localhost:11434/v1`).
    pub base_url: String,
    /// API key (optional, some local providers don't need one).
    pub api_key: Option<String>,
    /// Environment variable name to read the API key from.
    pub api_key_env: Option<String>,
    /// Whether to probe the `/v1/models` endpoint for model discovery.
    #[serde(default)]
    pub fetch_models: bool,
    /// Statically configured models.
    #[serde(default)]
    pub models: Vec<ProviderModelConfig>,
}

/// A model entry within a `[[providers]]` section.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProviderModelConfig {
    /// Model identifier sent to the API (e.g. `"llama3.1:70b"`).
    pub id: String,
    /// Human-readable display name (e.g. `"Llama 3.1 70B"`).
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteHostConfig {
    pub name: String,
    pub hostname: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub user: String,
    pub identity_file: Option<String>,
    #[serde(default = "default_remote_base_path")]
    pub remote_base_path: String,
    pub daemon_port: Option<u16>,
    pub mosh: Option<bool>,
    pub mosh_server_path: Option<String>,
}

fn default_ssh_port() -> u16 {
    22
}

fn default_remote_base_path() -> String {
    "~/arbor-outposts".to_owned()
}

pub struct LoadedArborConfig {
    pub config: ArborConfig,
    pub notices: Vec<String>,
}

pub trait AppConfigStore: Send + Sync {
    fn config_path(&self) -> PathBuf;
    fn config_last_modified(&self) -> Option<SystemTime>;
    fn load_or_create_config(&self) -> LoadedArborConfig;
    fn append_remote_host(&self, host: &RemoteHostConfig) -> Result<(), StoreError>;
    fn remove_remote_host(&self, name: &str) -> Result<(), StoreError>;
    fn load_repo_config(&self, repo_root: &Path) -> Option<RepoConfig>;
    fn save_repo_presets(
        &self,
        repo_root: &Path,
        presets: &[RepoPresetConfig],
    ) -> Result<(), StoreError>;
    fn remove_repo_preset(&self, repo_root: &Path, name: &str) -> Result<(), StoreError>;
    fn save_scalar_settings(&self, settings: &[(&str, Option<&str>)]) -> Result<(), StoreError>;
    fn save_daemon_bind_mode(&self, bind_mode: Option<&str>) -> Result<(), StoreError>;
    fn save_agent_presets(&self, presets: &[AgentPresetConfig]) -> Result<(), StoreError>;
}

pub struct FileAppConfigStore {
    path: PathBuf,
}

impl FileAppConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Default for FileAppConfigStore {
    fn default() -> Self {
        Self::new(default_config_path())
    }
}

impl AppConfigStore for FileAppConfigStore {
    fn config_path(&self) -> PathBuf {
        self.path.clone()
    }

    fn config_last_modified(&self) -> Option<SystemTime> {
        config_last_modified(&self.path)
    }

    fn load_or_create_config(&self) -> LoadedArborConfig {
        load_or_create_config_at(&self.path)
    }

    fn append_remote_host(&self, host: &RemoteHostConfig) -> Result<(), StoreError> {
        append_remote_host_at(&self.path, host)
    }

    fn remove_remote_host(&self, name: &str) -> Result<(), StoreError> {
        remove_remote_host_at(&self.path, name)
    }

    fn load_repo_config(&self, repo_root: &Path) -> Option<RepoConfig> {
        load_repo_config(repo_root)
    }

    fn save_repo_presets(
        &self,
        repo_root: &Path,
        presets: &[RepoPresetConfig],
    ) -> Result<(), StoreError> {
        save_repo_presets(repo_root, presets)
    }

    fn remove_repo_preset(&self, repo_root: &Path, name: &str) -> Result<(), StoreError> {
        remove_repo_preset(repo_root, name)
    }

    fn save_scalar_settings(&self, settings: &[(&str, Option<&str>)]) -> Result<(), StoreError> {
        save_scalar_settings_at(&self.path, settings)
    }

    fn save_daemon_bind_mode(&self, bind_mode: Option<&str>) -> Result<(), StoreError> {
        save_daemon_bind_mode_at(&self.path, bind_mode)
    }

    fn save_agent_presets(&self, presets: &[AgentPresetConfig]) -> Result<(), StoreError> {
        save_agent_presets_at(&self.path, presets)
    }
}

pub fn default_app_config_store() -> Arc<dyn AppConfigStore> {
    Arc::new(FileAppConfigStore::default())
}

fn load_or_create_config_at(path: &Path) -> LoadedArborConfig {
    let mut notices = Vec::new();

    if let Err(error) = ensure_config_file_exists(path) {
        notices.push(error.to_string());
    }

    let parsed = Config::builder()
        .add_source(File::from(path).required(false))
        .build()
        .and_then(|settings| settings.try_deserialize::<ArborConfig>());

    match parsed {
        Ok(config) => LoadedArborConfig { config, notices },
        Err(error) => {
            notices.push(format!("failed to parse {}: {error}", path.display()));
            LoadedArborConfig {
                config: ArborConfig::default(),
                notices,
            }
        },
    }
}

pub fn config_last_modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
}

fn ensure_config_file_exists(path: &Path) -> Result<(), StoreError> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| StoreError::CreateDir {
            path: parent.display().to_string(),
            source,
        })?;
    }

    fs::write(path, DEFAULT_CONFIG_CONTENT).map_err(|source| StoreError::Write {
        path: path.display().to_string(),
        source,
    })
}

fn default_config_path() -> PathBuf {
    match env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(CONFIG_RELATIVE_PATH),
        Err(_) => PathBuf::from(CONFIG_RELATIVE_PATH),
    }
}

fn append_remote_host_at(path: &Path, host: &RemoteHostConfig) -> Result<(), StoreError> {
    let path_str = path.display().to_string();
    let content = fs::read_to_string(path).map_err(|source| StoreError::Read {
        path: path_str.clone(),
        source,
    })?;
    let mut doc: DocumentMut = content.parse().map_err(|source| StoreError::TomlParse {
        path: path_str.clone(),
        source,
    })?;

    let arr = doc
        .entry("remote_hosts")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or_else(|| StoreError::Other("remote_hosts is not an array of tables".to_owned()))?;

    let mut table = toml_edit::Table::new();
    table.insert("name", toml_edit::value(&host.name));
    table.insert("hostname", toml_edit::value(&host.hostname));
    table.insert("user", toml_edit::value(&host.user));
    if host.port != 22 {
        table.insert("port", toml_edit::value(i64::from(host.port)));
    }
    if let Some(ref identity_file) = host.identity_file {
        table.insert("identity_file", toml_edit::value(identity_file));
    }
    if host.remote_base_path != "~/arbor-outposts" {
        table.insert("remote_base_path", toml_edit::value(&host.remote_base_path));
    }
    if let Some(daemon_port) = host.daemon_port {
        table.insert("daemon_port", toml_edit::value(i64::from(daemon_port)));
    }

    arr.push(table);

    fs::write(path, doc.to_string()).map_err(|source| StoreError::Write {
        path: path_str,
        source,
    })
}

fn remove_remote_host_at(path: &Path, name: &str) -> Result<(), StoreError> {
    let path_str = path.display().to_string();
    let content = fs::read_to_string(path).map_err(|source| StoreError::Read {
        path: path_str.clone(),
        source,
    })?;
    let mut doc: DocumentMut = content.parse().map_err(|source| StoreError::TomlParse {
        path: path_str.clone(),
        source,
    })?;

    if let Some(arr) = doc
        .get_mut("remote_hosts")
        .and_then(|v| v.as_array_of_tables_mut())
    {
        let mut index_to_remove = None;
        for (i, table) in arr.iter().enumerate() {
            if table.get("name").and_then(|v| v.as_str()) == Some(name) {
                index_to_remove = Some(i);
                break;
            }
        }
        if let Some(idx) = index_to_remove {
            arr.remove(idx);
        }
        if arr.is_empty() {
            doc.remove("remote_hosts");
        }
    }

    fs::write(path, doc.to_string()).map_err(|source| StoreError::Write {
        path: path_str,
        source,
    })
}

// ── Per-repository config (arbor.toml) ───────────────────────────────

pub type RepoConfig = repo_config::RepoConfig;
pub type RepoPresetConfig = repo_config::RepoPresetConfig;

pub fn load_repo_config(repo_root: &Path) -> Option<RepoConfig> {
    repo_config::load_repo_config(repo_root)
}

pub fn save_repo_presets(repo_root: &Path, presets: &[RepoPresetConfig]) -> Result<(), StoreError> {
    let path = repo_root.join("arbor.toml");
    let path_str = path.display().to_string();
    let content = if path.exists() {
        fs::read_to_string(&path).map_err(|source| StoreError::Read {
            path: path_str.clone(),
            source,
        })?
    } else {
        String::new()
    };
    let mut doc: DocumentMut = content.parse().map_err(|source| StoreError::TomlParse {
        path: path_str.clone(),
        source,
    })?;

    doc.remove("presets");

    if !presets.is_empty() {
        let mut arr = toml_edit::ArrayOfTables::new();
        for preset in presets {
            let mut table = toml_edit::Table::new();
            table.insert("name", toml_edit::value(&preset.name));
            table.insert("icon", toml_edit::value(&preset.icon));
            table.insert("command", toml_edit::value(&preset.command));
            arr.push(table);
        }
        doc.insert("presets", toml_edit::Item::ArrayOfTables(arr));
    }

    fs::write(&path, doc.to_string()).map_err(|source| StoreError::Write {
        path: path_str,
        source,
    })
}

pub fn remove_repo_preset(repo_root: &Path, name: &str) -> Result<(), StoreError> {
    let path = repo_root.join("arbor.toml");
    let path_str = path.display().to_string();
    let content = fs::read_to_string(&path).map_err(|source| StoreError::Read {
        path: path_str.clone(),
        source,
    })?;
    let mut doc: DocumentMut = content.parse().map_err(|source| StoreError::TomlParse {
        path: path_str.clone(),
        source,
    })?;

    if let Some(arr) = doc
        .get_mut("presets")
        .and_then(|v| v.as_array_of_tables_mut())
    {
        let mut index_to_remove = None;
        for (i, table) in arr.iter().enumerate() {
            if table.get("name").and_then(|v| v.as_str()) == Some(name) {
                index_to_remove = Some(i);
                break;
            }
        }
        if let Some(idx) = index_to_remove {
            arr.remove(idx);
        }
        if arr.is_empty() {
            doc.remove("presets");
        }
    }

    fs::write(&path, doc.to_string()).map_err(|source| StoreError::Write {
        path: path_str,
        source,
    })
}

fn save_scalar_settings_at(
    path: &Path,
    settings: &[(&str, Option<&str>)],
) -> Result<(), StoreError> {
    let path_str = path.display().to_string();
    let content = fs::read_to_string(path).map_err(|source| StoreError::Read {
        path: path_str.clone(),
        source,
    })?;
    let mut doc: DocumentMut = content.parse().map_err(|source| StoreError::TomlParse {
        path: path_str.clone(),
        source,
    })?;

    for &(key, value) in settings {
        match value {
            Some(v) if !v.is_empty() => {
                doc.insert(key, toml_edit::value(v));
            },
            _ => {
                doc.remove(key);
            },
        }
    }

    fs::write(path, doc.to_string()).map_err(|source| StoreError::Write {
        path: path_str,
        source,
    })
}

fn save_daemon_bind_mode_at(path: &Path, bind_mode: Option<&str>) -> Result<(), StoreError> {
    let path_str = path.display().to_string();
    let content = fs::read_to_string(path).map_err(|source| StoreError::Read {
        path: path_str.clone(),
        source,
    })?;
    let mut doc: DocumentMut = content.parse().map_err(|source| StoreError::TomlParse {
        path: path_str.clone(),
        source,
    })?;

    match bind_mode.filter(|value| !value.trim().is_empty()) {
        Some(value) => {
            let daemon_table = doc
                .entry("daemon")
                .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_mut()
                .ok_or_else(|| StoreError::Other("daemon is not a table".to_owned()))?;
            daemon_table.insert("bind", toml_edit::value(value));
        },
        None => {
            if let Some(daemon_table) = doc.get_mut("daemon").and_then(|item| item.as_table_mut()) {
                daemon_table.remove("bind");
                if daemon_table.is_empty() {
                    doc.remove("daemon");
                }
            }
        },
    }

    fs::write(path, doc.to_string()).map_err(|source| StoreError::Write {
        path: path_str,
        source,
    })
}

fn save_agent_presets_at(path: &Path, presets: &[AgentPresetConfig]) -> Result<(), StoreError> {
    let path_str = path.display().to_string();
    let content = fs::read_to_string(path).map_err(|source| StoreError::Read {
        path: path_str.clone(),
        source,
    })?;
    let mut doc: DocumentMut = content.parse().map_err(|source| StoreError::TomlParse {
        path: path_str.clone(),
        source,
    })?;

    doc.remove("agent_presets");

    if !presets.is_empty() {
        let mut arr = toml_edit::ArrayOfTables::new();
        for preset in presets {
            let mut table = toml_edit::Table::new();
            table.insert("key", toml_edit::value(&preset.key));
            table.insert("command", toml_edit::value(&preset.command));
            arr.push(table);
        }
        doc.insert("agent_presets", toml_edit::Item::ArrayOfTables(arr));
    }

    fs::write(path, doc.to_string()).map_err(|source| StoreError::Write {
        path: path_str,
        source,
    })
}
