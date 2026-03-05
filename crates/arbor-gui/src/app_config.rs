use {
    config::{Config, File},
    serde::Deserialize,
    std::{
        env, fs,
        path::{Path, PathBuf},
        time::SystemTime,
    },
};

const CONFIG_RELATIVE_PATH: &str = ".config/arbor/config.toml";
const DEFAULT_CONFIG_CONTENT: &str = r#"# Arbor configuration
# terminal_backend = "embedded" # embedded | alacritty | ghostty
# theme = "one-dark"            # one-dark | ayu-dark | gruvbox-dark
# daemon_url = "http://127.0.0.1:8787" # arbor-httpd base URL
"#;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ArborConfig {
    pub terminal_backend: Option<String>,
    pub theme: Option<String>,
    pub daemon_url: Option<String>,
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
