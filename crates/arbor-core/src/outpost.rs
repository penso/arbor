use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteHost {
    pub name: String,
    pub hostname: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub user: String,
    pub identity_file: Option<String>,
    #[serde(default = "default_remote_base_path")]
    pub remote_base_path: String,
    pub daemon_port: Option<u16>,
    #[serde(default)]
    pub mosh: Option<bool>,
    pub mosh_server_path: Option<String>,
}

fn default_ssh_port() -> u16 {
    22
}

fn default_remote_base_path() -> String {
    "~/arbor-outposts".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutpostRecord {
    pub id: String,
    pub host_name: String,
    pub local_repo_root: String,
    pub remote_path: String,
    pub clone_url: String,
    pub branch: String,
    pub label: String,
    pub has_remote_daemon: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OutpostStatus {
    #[default]
    Available,
    NotCloned,
    Unreachable,
    Provisioning,
}
