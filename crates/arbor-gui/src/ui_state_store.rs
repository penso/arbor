use {
    serde::{Deserialize, Serialize},
    std::{
        env, fs,
        path::{Path, PathBuf},
    },
};

const UI_STATE_STORE_RELATIVE_PATH: &str = ".arbor/ui-state.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UiState {
    pub left_pane_width: Option<i32>,
    pub right_pane_width: Option<i32>,
    pub window: Option<WindowGeometry>,
    pub left_pane_visible: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub trait UiStateStore: Send + Sync {
    fn load(&self) -> Result<UiState, String>;
    fn save(&self, state: &UiState) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub struct JsonUiStateStore {
    path: PathBuf,
}

impl JsonUiStateStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Default for JsonUiStateStore {
    fn default() -> Self {
        Self::new(default_ui_state_store_path())
    }
}

impl UiStateStore for JsonUiStateStore {
    fn load(&self) -> Result<UiState, String> {
        if !self.path.exists() {
            return Ok(UiState::default());
        }

        let raw = fs::read_to_string(&self.path).map_err(|error| {
            format!(
                "failed to read UI state file `{}`: {error}",
                self.path.display()
            )
        })?;

        serde_json::from_str(&raw).map_err(|error| {
            format!(
                "failed to parse UI state file `{}`: {error}",
                self.path.display()
            )
        })
    }

    fn save(&self, state: &UiState) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create UI state directory `{}`: {error}",
                    parent.display()
                )
            })?;
        }

        let payload = serde_json::to_string_pretty(state).map_err(|error| {
            format!(
                "failed to serialize UI state for `{}`: {error}",
                self.path.display()
            )
        })?;

        fs::write(&self.path, payload).map_err(|error| {
            format!(
                "failed to write UI state file `{}`: {error}",
                self.path.display()
            )
        })
    }
}

pub fn default_ui_state_store() -> Box<dyn UiStateStore> {
    Box::new(JsonUiStateStore::default())
}

pub fn load_startup_state() -> UiState {
    let store = JsonUiStateStore::default();
    store.load().unwrap_or_default()
}

fn default_ui_state_store_path() -> PathBuf {
    resolve_home_relative(UI_STATE_STORE_RELATIVE_PATH)
}

fn resolve_home_relative(relative_path: &str) -> PathBuf {
    home_dir().join(relative_path)
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}
