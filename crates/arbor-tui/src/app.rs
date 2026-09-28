use {
    crate::{
        hooks::{ActionTab, Config},
        widgets::list_detail::ListDetailState,
    },
    arbor_daemon_client::AgentSessionDto,
    std::time::Instant,
};

pub struct App {
    pub running: bool,
    pub connected: bool,
    pub last_poll: Option<Instant>,
    pub agents: Vec<AgentSessionDto>,
    pub agents_state: ListDetailState,
    pub config: Config,
    pub pane_output: Option<String>,
    pub input_mode: bool,
    pub input_buffer: String,
    pub table_collapsed: bool,
    pub meta_collapsed: bool,
    pub show_help: bool,
    pub show_detail: bool,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load();
        Self {
            running: true,
            connected: false,
            last_poll: None,
            agents: Vec::new(),
            agents_state: ListDetailState::new(),
            config,
            pane_output: None,
            input_mode: false,
            input_buffer: String::new(),
            table_collapsed: false,
            meta_collapsed: false,
            show_help: false,
            show_detail: false,
        }
    }

    pub fn apply_daemon_data(&mut self, data: Vec<crate::client::DaemonData>) {
        use crate::client::DaemonData;
        for d in data {
            match d {
                DaemonData::Health(ok) => {
                    self.connected = ok;
                    self.last_poll = Some(Instant::now());
                },
                DaemonData::Agents(new_agents) => {
                    self.agents_state.set_count(new_agents.len());
                    self.agents = new_agents;
                },
                DaemonData::PaneOutput(output) => {
                    self.pane_output = output;
                },
            }
        }
    }

    pub fn last_poll_secs(&self) -> Option<u64> {
        self.last_poll.map(|t| t.elapsed().as_secs())
    }

    pub fn current_list_state_mut(&mut self) -> &mut ListDetailState {
        &mut self.agents_state
    }

    pub fn current_action_tab(&self) -> ActionTab {
        ActionTab::Agents
    }

    pub fn selected_env_vars(&self) -> Vec<(&str, String)> {
        if let Some(agent) = self.agents.get(self.agents_state.selected) {
            vec![
                ("ARBOR_SESSION_ID", agent.session_id.clone()),
                ("ARBOR_CWD", agent.cwd.clone()),
                ("ARBOR_STATE", agent.state.clone()),
                ("ARBOR_TAB", "agents".to_owned()),
            ]
        } else {
            Vec::new()
        }
    }

    pub fn quit(&mut self) {
        self.running = false;
    }
}
