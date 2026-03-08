use {
    config::{Config, File},
    serde::Deserialize,
    std::{
        env, fs,
        path::{Path, PathBuf},
        time::SystemTime,
    },
    toml_edit::DocumentMut,
};

const CONFIG_RELATIVE_PATH: &str = ".config/arbor/config.toml";
const DEFAULT_CONFIG_CONTENT: &str = r#"# Arbor configuration
# terminal_backend = "embedded" # embedded | alacritty | ghostty
# theme = "one-dark"            # one-dark | ayu-dark | gruvbox-dark | dracula | solarized-light | everforest-dark | catppuccin | catppuccin-latte | ethereal | flexoki-light | hackerman | kanagawa | matte-black | miasma | nord | osaka-jade | ristretto | rose-pine | tokyo-night | vantablack | white | retrobox-classic | tokyonight-day | tokyonight-classic | zellner
# daemon_url = "http://127.0.0.1:8787" # arbor-httpd base URL
# notifications = true
#
# [[agent_presets]]
# key = "codex"     # codex | claude | opencode | copilot
# command = "codex"
#
# [[agent_presets]]
# key = "claude"
# command = "claude"
#
# [[agent_presets]]
# key = "opencode"
# command = "opencode"

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
    pub theme: Option<String>,
    pub daemon_url: Option<String>,
    pub notifications: Option<bool>,
    pub preferred_editor: Option<String>,
    pub agent_presets: Vec<AgentPresetConfig>,
    pub remote_hosts: Vec<RemoteHostConfig>,
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

pub fn load_or_create_config() -> LoadedArborConfig {
    let path = default_config_path();
    let mut notices = Vec::new();

    if let Err(error) = ensure_config_file_exists(&path) {
        notices.push(error);
    }

    let parsed = Config::builder()
        .add_source(File::from(path.as_path()).required(false))
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

pub fn config_path() -> PathBuf {
    default_config_path()
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

pub fn append_remote_host(host: &RemoteHostConfig) -> Result<(), String> {
    let path = config_path();
    let content =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
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

    fs::write(&path, doc.to_string())
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

pub fn remove_remote_host(name: &str) -> Result<(), String> {
    let path = config_path();
    let content =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
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

    fs::write(&path, doc.to_string())
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

// ── Per-repository config (arbor.toml) ───────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RepoConfig {
    pub presets: Vec<RepoPresetConfig>,
    pub processes: Vec<ProcessConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProcessConfig {
    pub name: String,
    pub command: String,
    pub working_dir: Option<String>,
    pub auto_start: Option<bool>,
    pub auto_restart: Option<bool>,
    pub restart_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RepoPresetConfig {
    pub name: String,
    pub icon: String,
    pub command: String,
}

pub fn load_repo_config(repo_root: &Path) -> Option<RepoConfig> {
    let path = repo_root.join("arbor.toml");
    if !path.exists() {
        return None;
    }
    let config: RepoConfig = Config::builder()
        .add_source(File::from(path.as_path()).required(false))
        .build()
        .ok()
        .and_then(|settings| settings.try_deserialize().ok())?;
    Some(config)
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

pub fn save_agent_presets(presets: &[AgentPresetConfig]) -> Result<(), String> {
    let path = config_path();
    let content =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
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

    fs::write(&path, doc.to_string())
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}
