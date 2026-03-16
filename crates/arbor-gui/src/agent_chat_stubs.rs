/// Stub implementations for agent chat methods when the `agent-chat` feature is
/// disabled.  These no-op methods keep all call sites compiling without pulling
/// in the real agent chat implementation.
use super::*;

impl ArborWindow {
    pub(crate) fn spawn_agent_chat(
        &mut self,
        _kind: AgentPresetKind,
        _model_id: Option<String>,
        _cx: &mut Context<Self>,
    ) {
    }

    pub(crate) fn spawn_api_agent_chat(
        &mut self,
        _provider_name: &str,
        _model_id: &str,
        _transport: terminal_daemon_http::AgentChatTransport,
        _cx: &mut Context<Self>,
    ) {
    }

    pub(crate) fn probe_provider_models(&mut self, _cx: &mut Context<Self>) {}

    pub(crate) fn restore_agent_chat_sessions(&mut self, _cx: &mut Context<Self>) {}

    pub(crate) fn handle_agent_chat_key_down(
        &mut self,
        _local_id: u64,
        _event: &KeyDownEvent,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }

    pub(crate) fn send_agent_chat_message(&mut self, _local_id: u64, _cx: &mut Context<Self>) {}

    pub(crate) fn cancel_agent_chat(&mut self, _local_id: u64, _cx: &mut Context<Self>) {}
}
