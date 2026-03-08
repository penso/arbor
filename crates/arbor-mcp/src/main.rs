use {
    anyhow::Result,
    rmcp::{
        ServerHandler, ServiceExt,
        handler::server::{router::tool::ToolRouter, wrapper::Parameters},
        model::{Implementation, ServerCapabilities, ServerInfo},
        schemars::JsonSchema,
        tool, tool_handler, tool_router,
    },
    serde::Deserialize,
    std::env,
};

const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:8787";

fn daemon_url() -> String {
    env::var("ARBOR_DAEMON_URL").unwrap_or_else(|_| DEFAULT_DAEMON_URL.to_owned())
}

fn api_get(path: &str) -> String {
    let url = format!("{}/api/v1{path}", daemon_url());
    match ureq::get(&url).header("Accept", "application/json").call() {
        Ok(response) => response
            .into_body()
            .read_to_string()
            .unwrap_or_else(|e| format!("{{\"error\": \"failed to read body: {e}\"}}")),
        Err(e) => format!("{{\"error\": \"request failed: {e}\"}}"),
    }
}

fn api_post_empty(path: &str) -> String {
    let url = format!("{}/api/v1{path}", daemon_url());
    match ureq::post(&url)
        .header("Accept", "application/json")
        .send_empty()
    {
        Ok(response) => {
            if response.status() == 204 {
                return "ok".to_owned();
            }
            response
                .into_body()
                .read_to_string()
                .unwrap_or_else(|e| format!("{{\"error\": \"failed to read body: {e}\"}}"))
        },
        Err(e) => format!("{{\"error\": \"request failed: {e}\"}}"),
    }
}

fn api_post_json(path: &str, body: &str) -> String {
    let url = format!("{}/api/v1{path}", daemon_url());
    match ureq::post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send(body.as_bytes())
    {
        Ok(response) => {
            if response.status() == 204 {
                return "ok".to_owned();
            }
            response
                .into_body()
                .read_to_string()
                .unwrap_or_else(|e| format!("{{\"error\": \"failed to read body: {e}\"}}"))
        },
        Err(e) => format!("{{\"error\": \"request failed: {e}\"}}"),
    }
}

/// Arbor MCP server — exposes process management, terminal sessions,
/// and worktree information to AI agents via the Model Context Protocol.
#[derive(Debug, Clone)]
pub struct ArborMcp {
    tool_router: ToolRouter<Self>,
}

impl Default for ArborMcp {
    fn default() -> Self {
        Self::new()
    }
}

impl ArborMcp {
    /// Create a new ArborMcp server instance.
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

/// Input for process name operations.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProcessNameInput {
    /// Name of the process to act on.
    pub name: String,
}

/// Input for reading terminal output.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TerminalReadInput {
    /// Terminal session ID to read from.
    pub session_id: String,
}

/// Input for writing terminal input.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TerminalWriteInput {
    /// Terminal session ID to write to.
    pub session_id: String,
    /// Data to send to the terminal.
    pub data: String,
}

/// Input for listing changed files.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChangesInput {
    /// Worktree path to list changed files for.
    pub path: String,
}

#[tool_router(router = tool_router)]
impl ArborMcp {
    /// List all managed processes defined in arbor.toml with their current status.
    #[tool(
        description = "List all managed processes defined in arbor.toml and their current status (running, stopped, crashed, restarting)"
    )]
    pub async fn list_processes(&self) -> String {
        api_get("/processes")
    }

    /// Start a managed process by name.
    #[tool(description = "Start a managed process by name")]
    pub async fn start_process(&self, input: Parameters<ProcessNameInput>) -> String {
        let path = format!("/processes/{}/start", urlencoding(&input.0.name));
        api_post_empty(&path)
    }

    /// Stop a managed process by name.
    #[tool(description = "Stop a managed process by name")]
    pub async fn stop_process(&self, input: Parameters<ProcessNameInput>) -> String {
        let path = format!("/processes/{}/stop", urlencoding(&input.0.name));
        api_post_empty(&path)
    }

    /// Restart a managed process by name.
    #[tool(description = "Restart a managed process by name")]
    pub async fn restart_process(&self, input: Parameters<ProcessNameInput>) -> String {
        let path = format!("/processes/{}/restart", urlencoding(&input.0.name));
        api_post_empty(&path)
    }

    /// Read recent output from a terminal session.
    #[tool(description = "Read recent output from a terminal session by session_id")]
    pub async fn read_terminal_output(&self, input: Parameters<TerminalReadInput>) -> String {
        let path = format!("/terminals/{}/snapshot", urlencoding(&input.0.session_id));
        api_get(&path)
    }

    /// Write input to a terminal session.
    #[tool(description = "Send input data to a terminal session by session_id")]
    pub async fn write_terminal_input(&self, input: Parameters<TerminalWriteInput>) -> String {
        let path = format!("/terminals/{}/write", urlencoding(&input.0.session_id));
        let body = serde_json::json!({ "data": input.0.data }).to_string();
        api_post_json(&path, &body)
    }

    /// List changed files in a worktree.
    #[tool(description = "List files that have been changed in a worktree (git diff)")]
    pub async fn list_changed_files(&self, input: Parameters<ChangesInput>) -> String {
        let path = format!("/worktrees/changes?path={}", urlencoding(&input.0.path));
        api_get(&path)
    }
}

#[tool_handler]
impl ServerHandler for ArborMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::from_build_env();
        info.instructions = Some(
            "Arbor MCP server. Provides access to managed processes, terminal sessions, \
             worktrees, and changed files via the arbor-httpd REST API."
                .to_owned(),
        );
        info
    }
}

fn urlencoding(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            },
            _ => {
                result.push('%');
                result.push_str(&format!("{byte:02X}"));
            },
        }
    }
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("arbor-mcp starting (daemon: {})", daemon_url());

    let service = ArborMcp::new().serve(rmcp::transport::io::stdio()).await?;

    service.waiting().await?;
    Ok(())
}
