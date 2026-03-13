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

pub fn save_history(entries: &[ConnectionHistoryEntry]) -> Result<(), String> {
    let path = history_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create connection history directory `{}`: {error}",
                parent.display()
            )
        })?;
    }
    let json = serde_json::to_string_pretty(entries).map_err(|error| {
        format!(
            "failed to serialize connection history `{}`: {error}",
            path.display()
        )
    })?;
    fs::write(&path, json).map_err(|error| {
        format!(
            "failed to write connection history `{}`: {error}",
            path.display()
        )
    })
}

pub fn updated_history_entries(
    entries: &[ConnectionHistoryEntry],
    address: &str,
    label: Option<&str>,
) -> Vec<ConnectionHistoryEntry> {
    let mut entries = entries.to_vec();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Some(existing) = entries.iter_mut().find(|entry| entry.address == address) {
        existing.last_connected = now;
        if let Some(label) = label {
            existing.label = Some(label.to_owned());
        }
    } else {
        entries.push(ConnectionHistoryEntry {
            address: address.to_owned(),
            label: label.map(|value| value.to_owned()),
            last_connected: now,
        });
    }

    entries.sort_by(|left, right| right.last_connected.cmp(&left.last_connected));
    entries.truncate(MAX_ENTRIES);
    entries
}

pub fn history_without_address(
    entries: &[ConnectionHistoryEntry],
    address: &str,
) -> Vec<ConnectionHistoryEntry> {
    let mut entries = entries.to_vec();
    entries.retain(|entry| entry.address != address);
    entries
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

pub fn save_tokens(tokens: &HashMap<String, String>) -> Result<(), String> {
    let path = tokens_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create daemon auth token directory `{}`: {error}",
                parent.display()
            )
        })?;
    }
    let json = serde_json::to_string_pretty(tokens).map_err(|error| {
        format!(
            "failed to serialize daemon auth tokens `{}`: {error}",
            path.display()
        )
    })?;
    fs::write(&path, json).map_err(|error| {
        format!(
            "failed to write daemon auth tokens `{}`: {error}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionHistoryEntry, MAX_ENTRIES, history_without_address, updated_history_entries,
    };

    #[test]
    fn updated_history_entries_promotes_existing_addresses_and_caps_length() {
        let mut entries: Vec<ConnectionHistoryEntry> = (0..MAX_ENTRIES)
            .map(|index| ConnectionHistoryEntry {
                address: format!("host-{index}"),
                label: None,
                last_connected: index as u64,
            })
            .collect();
        entries.reverse();

        let updated = updated_history_entries(&entries, "host-3", Some("dev box"));
        assert_eq!(updated.len(), MAX_ENTRIES);
        assert_eq!(updated[0].address, "host-3");
        assert_eq!(updated[0].label.as_deref(), Some("dev box"));

        let inserted = updated_history_entries(&updated, "new-host", None);
        assert_eq!(inserted.len(), MAX_ENTRIES);
        assert!(inserted.iter().any(|entry| entry.address == "new-host"));
        assert!(inserted.iter().all(|entry| entry.address != "host-0"));
    }

    #[test]
    fn history_without_address_removes_the_target_entry() {
        let entries = vec![
            ConnectionHistoryEntry {
                address: "one".to_owned(),
                label: None,
                last_connected: 1,
            },
            ConnectionHistoryEntry {
                address: "two".to_owned(),
                label: Some("secondary".to_owned()),
                last_connected: 2,
            },
        ];

        let filtered = history_without_address(&entries, "one");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].address, "two");
    }
}
