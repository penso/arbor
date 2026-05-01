use {
    crate::capture,
    arbor_daemon_client::{AgentSessionDto, DaemonClient},
    std::{
        sync::{Arc, Mutex, mpsc},
        time::Duration,
    },
};

pub enum DaemonData {
    Health(bool),
    Agents(Vec<AgentSessionDto>),
    PaneOutput(Option<String>),
}

pub struct DaemonPoller {
    rx: mpsc::Receiver<DaemonData>,
    capture_request: Arc<Mutex<Option<AgentSessionDto>>>,
}

impl DaemonPoller {
    pub fn start(port: u16, poll_interval: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let base_url = format!("http://127.0.0.1:{}", port);
        let capture_request: Arc<Mutex<Option<AgentSessionDto>>> = Arc::new(Mutex::new(None));

        let tx_daemon = tx.clone();
        std::thread::spawn(move || {
            let client = DaemonClient::new(&base_url);
            loop {
                let connected = client.health().is_ok();
                let _ = tx_daemon.send(DaemonData::Health(connected));

                if connected && let Ok(agents) = client.list_agent_activity() {
                    let _ = tx_daemon.send(DaemonData::Agents(agents));
                }

                std::thread::sleep(poll_interval);
            }
        });

        let capture_request_clone = Arc::clone(&capture_request);
        let capture_interval = Duration::from_millis(500);
        std::thread::spawn(move || {
            loop {
                let agent = capture_request_clone
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();

                let result = agent.and_then(|a| {
                    let backend = capture::capture_for(&a)?;
                    backend.capture(&a)
                });
                let _ = tx.send(DaemonData::PaneOutput(result));

                std::thread::sleep(capture_interval);
            }
        });

        Self {
            rx,
            capture_request,
        }
    }

    pub fn request_capture(&self, agent: &AgentSessionDto) {
        if let Ok(mut guard) = self.capture_request.lock() {
            *guard = Some(agent.clone());
        }
    }

    pub fn clear_capture(&self) {
        if let Ok(mut guard) = self.capture_request.lock() {
            *guard = None;
        }
    }

    pub fn drain(&self) -> Vec<DaemonData> {
        let mut data = Vec::new();
        while let Ok(d) = self.rx.try_recv() {
            data.push(d);
        }
        data
    }
}
