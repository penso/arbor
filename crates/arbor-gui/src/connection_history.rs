use {
    serde::{Deserialize, Serialize},
    std::{
        collections::HashMap,
        env, fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    },
};

const MAX_ENTRIES: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionHistoryEntry {
    pub address: String,
    pub label: Option<String>,
    pub last_connected: u64,
}

fn history_path() -> PathBuf {
    match env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(".config/arbor/connection_history.json"),
        Err(_) => PathBuf::from(".config/arbor/connection_history.json"),
    }
}

pub fn load_history() -> Vec<ConnectionHistoryEntry> {
    let path = history_path();
    let Ok(data) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut entries: Vec<ConnectionHistoryEntry> = serde_json::from_str(&data).unwrap_or_default();
    entries.sort_by(|a, b| b.last_connected.cmp(&a.last_connected));
    entries
}

fn save_history(entries: &[ConnectionHistoryEntry]) {
    let path = history_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(json) = serde_json::to_string_pretty(entries) else {
        return;
    };
    let _ = fs::write(&path, json);
}

pub fn record_connection(address: &str, label: Option<&str>) {
    let mut entries = load_history();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Some(existing) = entries.iter_mut().find(|e| e.address == address) {
        existing.last_connected = now;
        if let Some(l) = label {
            existing.label = Some(l.to_owned());
        }
    } else {
        entries.push(ConnectionHistoryEntry {
            address: address.to_owned(),
            label: label.map(|l| l.to_owned()),
            last_connected: now,
        });
    }

    entries.sort_by(|a, b| b.last_connected.cmp(&a.last_connected));
    entries.truncate(MAX_ENTRIES);
    save_history(&entries);
}

pub fn remove_entry(address: &str) {
    let mut entries = load_history();
    entries.retain(|e| e.address != address);
    save_history(&entries);
}

fn tokens_path() -> PathBuf {
    match env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(".config/arbor/daemon_auth_tokens.json"),
        Err(_) => PathBuf::from(".config/arbor/daemon_auth_tokens.json"),
    }
}

pub fn load_tokens() -> HashMap<String, String> {
    let path = tokens_path();
    let Ok(data) = fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

pub fn save_tokens(tokens: &HashMap<String, String>) {
    let path = tokens_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(json) = serde_json::to_string_pretty(tokens) else {
        return;
    };
    let _ = fs::write(&path, json);
}
