use {arbor_daemon_client::AgentSessionDto, std::process::Command};

pub trait TerminalCapture: Send + Sync {
    fn capture(&self, agent: &AgentSessionDto) -> Option<String>;
    fn send_keys(&self, agent: &AgentSessionDto, text: &str) -> bool;
}

/// Returns the appropriate capture backend based on the agent's
/// `metadata.terminal.type` field, or `None` if no backend matches.
pub fn capture_for(agent: &AgentSessionDto) -> Option<Box<dyn TerminalCapture>> {
    let terminal = agent.metadata.as_ref()?.get("terminal")?;
    let terminal_type = terminal.get("type")?.as_str()?;
    match terminal_type {
        "tmux" => Some(Box::new(TmuxCapture)),
        _ => None,
    }
}

struct TmuxTarget<'a> {
    server: &'a str,
    pane_id: &'a str,
}

fn extract_tmux_target(agent: &AgentSessionDto) -> Option<TmuxTarget<'_>> {
    let terminal = agent.metadata.as_ref()?.get("terminal")?;
    Some(TmuxTarget {
        server: terminal.get("server")?.as_str()?,
        pane_id: terminal.get("pane_id")?.as_str()?,
    })
}

struct TmuxCapture;

impl TerminalCapture for TmuxCapture {
    fn capture(&self, agent: &AgentSessionDto) -> Option<String> {
        let target = extract_tmux_target(agent)?;
        let output = Command::new("tmux")
            .args([
                "-L",
                target.server,
                "capture-pane",
                "-p",
                "-e",
                "-t",
                target.pane_id,
            ])
            .output()
            .ok()?;
        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            None
        }
    }

    fn send_keys(&self, agent: &AgentSessionDto, text: &str) -> bool {
        let Some(target) = extract_tmux_target(agent) else {
            return false;
        };
        Command::new("tmux")
            .args([
                "-L",
                target.server,
                "send-keys",
                "-t",
                target.pane_id,
                text,
                "Enter",
            ])
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_with_tmux_metadata() -> AgentSessionDto {
        AgentSessionDto {
            session_id: "test-1".to_owned(),
            cwd: "/tmp/test".to_owned(),
            state: "working".to_owned(),
            updated_at_unix_ms: 0,
            metadata: Some(serde_json::json!({
                "terminal": { "type": "tmux", "server": "proj", "pane_id": "%1" }
            })),
        }
    }

    fn agent_without_metadata() -> AgentSessionDto {
        AgentSessionDto {
            session_id: "test-2".to_owned(),
            cwd: "/tmp/test".to_owned(),
            state: "idle".to_owned(),
            updated_at_unix_ms: 0,
            metadata: None,
        }
    }

    #[test]
    fn capture_for_returns_backend_for_tmux() {
        let agent = agent_with_tmux_metadata();
        assert!(capture_for(&agent).is_some());
    }

    #[test]
    fn capture_for_returns_none_for_unknown_type() {
        let mut agent = agent_with_tmux_metadata();
        agent.metadata = Some(serde_json::json!({
            "terminal": { "type": "kitty" }
        }));
        assert!(capture_for(&agent).is_none());
    }

    #[test]
    fn capture_for_returns_none_without_metadata() {
        assert!(capture_for(&agent_without_metadata()).is_none());
    }

    #[test]
    fn capture_for_returns_none_without_terminal_key() {
        let mut agent = agent_with_tmux_metadata();
        agent.metadata = Some(serde_json::json!({"git": {"branch": "main"}}));
        assert!(capture_for(&agent).is_none());
    }

    #[test]
    fn capture_returns_none_without_metadata() {
        let capturer = TmuxCapture;
        assert!(capturer.capture(&agent_without_metadata()).is_none());
    }

    #[test]
    fn extract_target_returns_server_and_pane() -> Result<(), Box<dyn std::error::Error>> {
        let agent = agent_with_tmux_metadata();
        let target = extract_tmux_target(&agent).ok_or("expected Some target")?;
        assert_eq!(target.server, "proj");
        assert_eq!(target.pane_id, "%1");
        Ok(())
    }

    #[test]
    fn extract_target_returns_none_for_missing_fields() {
        let mut agent = agent_with_tmux_metadata();
        agent.metadata = Some(serde_json::json!({"terminal": {"type": "tmux"}}));
        assert!(extract_tmux_target(&agent).is_none());
    }
}
