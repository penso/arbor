use {
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
# terminal_backend = "embedded" # embedded (in-app) | alacritty (external) | ghostty (external)
# embedded_terminal_engine = "alacritty" # alacritty | ghostty-vt-experimental
# embedded_shell = "/usr/bin/fish"  # shell for embedded terminal (defaults to $SHELL, then /bin/zsh)
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
    pub theme: Option<String>,
    pub daemon_url: Option<String>,
    pub notifications: Option<bool>,
    pub preferred_editor: Option<String>,
    pub agent_presets: Vec<AgentPresetConfig>,
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
    fn append_remote_host(&self, host: &RemoteHostConfig) -> Result<(), String>;
    fn remove_remote_host(&self, name: &str) -> Result<(), String>;
    fn load_repo_config(&self, repo_root: &Path) -> Option<RepoConfig>;
    fn save_repo_presets(
        &self,
        repo_root: &Path,
        presets: &[RepoPresetConfig],
    ) -> Result<(), String>;
    fn remove_repo_preset(&self, repo_root: &Path, name: &str) -> Result<(), String>;
    fn save_scalar_settings(&self, settings: &[(&str, Option<&str>)]) -> Result<(), String>;
    fn save_daemon_bind_mode(&self, bind_mode: Option<&str>) -> Result<(), String>;
    fn save_agent_presets(&self, presets: &[AgentPresetConfig]) -> Result<(), String>;
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

    fn append_remote_host(&self, host: &RemoteHostConfig) -> Result<(), String> {
        append_remote_host_at(&self.path, host)
    }

    fn remove_remote_host(&self, name: &str) -> Result<(), String> {
        remove_remote_host_at(&self.path, name)
    }

    fn load_repo_config(&self, repo_root: &Path) -> Option<RepoConfig> {
        load_repo_config(repo_root)
    }

    fn save_repo_presets(
        &self,
        repo_root: &Path,
        presets: &[RepoPresetConfig],
    ) -> Result<(), String> {
        save_repo_presets(repo_root, presets)
    }

    fn remove_repo_preset(&self, repo_root: &Path, name: &str) -> Result<(), String> {
        remove_repo_preset(repo_root, name)
    }

    fn save_scalar_settings(&self, settings: &[(&str, Option<&str>)]) -> Result<(), String> {
        save_scalar_settings_at(&self.path, settings)
    }

    fn save_daemon_bind_mode(&self, bind_mode: Option<&str>) -> Result<(), String> {
        save_daemon_bind_mode_at(&self.path, bind_mode)
    }

    fn save_agent_presets(&self, presets: &[AgentPresetConfig]) -> Result<(), String> {
        save_agent_presets_at(&self.path, presets)
    }
}

pub fn default_app_config_store() -> Arc<dyn AppConfigStore> {
    Arc::new(FileAppConfigStore::default())
}

fn load_or_create_config_at(path: &Path) -> LoadedArborConfig {
    let mut notices = Vec::new();

    if let Err(error) = ensure_config_file_exists(path) {
        notices.push(error);
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

fn ensure_config_file_exists(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create config directory `{}`: {error}",
                parent.display()
            )
        })?;
    }

    fs::write(path, DEFAULT_CONFIG_CONTENT)
        .map_err(|error| format!("failed to create config file `{}`: {error}", path.display()))
}

fn default_config_path() -> PathBuf {
    match env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(CONFIG_RELATIVE_PATH),
        Err(_) => PathBuf::from(CONFIG_RELATIVE_PATH),
    }
}

fn append_remote_host_at(path: &Path, host: &RemoteHostConfig) -> Result<(), String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;

    let arr = doc
        .entry("remote_hosts")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or_else(|| "remote_hosts is not an array of tables".to_owned())?;

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

    fs::write(path, doc.to_string()).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn remove_remote_host_at(path: &Path, name: &str) -> Result<(), String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;

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

    fs::write(path, doc.to_string()).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

// ── Per-repository config (arbor.toml) ───────────────────────────────

pub type RepoConfig = repo_config::RepoConfig;
pub type RepoPresetConfig = repo_config::RepoPresetConfig;

pub fn load_repo_config(repo_root: &Path) -> Option<RepoConfig> {
    repo_config::load_repo_config(repo_root)
}

pub fn save_repo_presets(repo_root: &Path, presets: &[RepoPresetConfig]) -> Result<(), String> {
    let path = repo_root.join("arbor.toml");
    let content = if path.exists() {
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?
    } else {
        String::new()
    };
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;

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

    fs::write(&path, doc.to_string())
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

pub fn remove_repo_preset(repo_root: &Path, name: &str) -> Result<(), String> {
    let path = repo_root.join("arbor.toml");
    let content =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;

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

    fs::write(&path, doc.to_string())
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn save_scalar_settings_at(path: &Path, settings: &[(&str, Option<&str>)]) -> Result<(), String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;

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

    fs::write(path, doc.to_string()).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn save_daemon_bind_mode_at(path: &Path, bind_mode: Option<&str>) -> Result<(), String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;

    match bind_mode.filter(|value| !value.trim().is_empty()) {
        Some(value) => {
            let daemon_table = doc
                .entry("daemon")
                .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_mut()
                .ok_or_else(|| "daemon is not a table".to_owned())?;
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

    fs::write(path, doc.to_string()).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn save_agent_presets_at(path: &Path, presets: &[AgentPresetConfig]) -> Result<(), String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;

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

    fs::write(path, doc.to_string()).map_err(|e| format!("failed to write {}: {e}", path.display()))
}
