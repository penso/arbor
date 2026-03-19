use {
    super::*,
    std::{
        collections::HashMap,
        sync::{Arc, Mutex, mpsc::Receiver},
        time::Duration,
    },
    sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, get_current_pid},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ProcessUsageSnapshot {
    cpu_percent: Option<u16>,
    memory_bytes: Option<u64>,
}

struct ProcessUsageSampler {
    pid: sysinfo::Pid,
    system: System,
}

impl ProcessUsageSampler {
    fn new() -> Option<Self> {
        let pid = get_current_pid().ok()?;
        let mut system = System::new();
        refresh_process_usage(&mut system, pid);
        Some(Self { pid, system })
    }

    fn snapshot(&mut self) -> ProcessUsageSnapshot {
        refresh_process_usage(&mut self.system, self.pid);

        let Some(process) = self.system.process(self.pid) else {
            return ProcessUsageSnapshot::default();
        };

        ProcessUsageSnapshot {
            cpu_percent: Some(process.cpu_usage().round().clamp(0.0, u16::MAX as f32) as u16),
            memory_bytes: Some(process.memory()),
        }
    }
}

fn refresh_process_usage(system: &mut System, pid: sysinfo::Pid) {
    let pids = [pid];
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&pids),
        ProcessRefreshKind::new().with_cpu().with_memory(),
    );
}

impl ArborWindow {
    pub(crate) fn start_terminal_poller(&mut self, cx: &mut Context<Self>) {
        let Some(poll_rx) = self.terminal_poll_rx.take() else {
            return;
        };
        let poll_rx = Arc::new(Mutex::new(poll_rx));

        cx.spawn(async move |this, cx| {
            loop {
                let wait_interval =
                    match this.update(cx, |this, _| this.terminal_background_sync_interval()) {
                        Ok(wait_interval) => wait_interval,
                        Err(_) => break,
                    };

                let poll_rx = Arc::clone(&poll_rx);
                let should_continue = cx
                    .background_spawn(async move {
                        let poll_rx = match poll_rx.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        wait_for_terminal_poller_event(&poll_rx, wait_interval)
                    })
                    .await;

                if !should_continue {
                    break;
                }

                let updated = this.update(cx, |this, cx| this.sync_running_terminals(cx));
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn start_log_poller(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(LOG_POLLER_INTERVAL);
                })
                .await;

                let updated = this.update(cx, |this, cx| {
                    let current_generation = this.log_buffer.generation();
                    if current_generation == this.log_generation {
                        return;
                    }
                    this.log_generation = current_generation;
                    this.log_entries = this.log_buffer.snapshot();
                    if this.log_auto_scroll && this.logs_tab_active {
                        this.log_scroll_handle.scroll_to_bottom();
                    }
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn start_worktree_auto_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(WORKTREE_AUTO_REFRESH_INTERVAL);
                })
                .await;

                let updated = this.update(cx, |this, cx| {
                    if this.worktree_stats_loading {
                        return;
                    }

                    let refresh = this.refresh_worktree_inventory(
                        cx,
                        WorktreeInventoryRefreshMode::PreserveTerminalState,
                    );
                    if this.active_outpost_index.is_some() {
                        this.refresh_remote_changed_files(cx);
                    } else {
                        this.refresh_changed_files(cx);
                    }
                    if refresh.visible_change() {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn start_github_pr_auto_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(GITHUB_PR_REFRESH_INTERVAL);
                })
                .await;

                let updated = this.update(cx, |this, cx| this.refresh_worktree_pull_requests(cx));
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn start_github_rate_limit_poller(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(Duration::from_secs(1));
                })
                .await;

                let updated = this.update(cx, |this, cx| {
                    if this.github_rate_limited_until.is_none() {
                        return;
                    }

                    if this.clear_expired_github_rate_limit() {
                        cx.notify();
                        return;
                    }

                    if this.github_rate_limit_remaining().is_some() {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn start_config_auto_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(CONFIG_AUTO_REFRESH_INTERVAL);
                })
                .await;

                let updated = this.update(cx, |this, cx| {
                    this.refresh_config_if_changed(cx);
                    this.refresh_repo_config_if_changed(cx);
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn has_active_loading_indicator(&self) -> bool {
        self.worktree_stats_loading
            || self.worktree_prs_loading
            || self.issue_lists.values().any(|state| state.loading)
            || self
                .create_modal
                .as_ref()
                .is_some_and(|modal| modal.managed_preview_loading)
    }

    pub(crate) fn ensure_loading_animation(&mut self, cx: &mut Context<Self>) {
        if self.loading_animation_active || !self.has_active_loading_indicator() {
            return;
        }

        self.loading_animation_active = true;

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(Duration::from_millis(100));
                })
                .await;

                let updated = this.update(cx, |this, cx| {
                    if !this.has_active_loading_indicator() {
                        this.loading_animation_active = false;
                        return false;
                    }

                    this.loading_animation_frame =
                        this.loading_animation_frame.wrapping_add(1) % LOADING_SPINNER_FRAMES.len();
                    cx.notify();
                    true
                });

                match updated {
                    Ok(true) => {},
                    Ok(false) | Err(_) => break,
                }
            }
        })
        .detach();
    }

    pub(crate) fn start_mdns_browser(&mut self, cx: &mut Context<Self>) {
        match mdns_browser::start_browsing() {
            Ok(browser) => {
                self.mdns_browser = Some(browser);
                tracing::info!("mDNS: browsing for _arbor._tcp services on the LAN");
            },
            Err(e) => {
                tracing::warn!("mDNS browsing unavailable: {e}");
                return;
            },
        }

        let local_hostname = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_default();

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(Duration::from_secs(2));
                })
                .await;

                let updated = this.update(cx, |this, cx| {
                    if let Some(browser) = &this.mdns_browser {
                        let events = browser.poll_updates();
                        let mut changed = false;
                        for event in events {
                            match event {
                                mdns_browser::MdnsEvent::Added(daemon) => {
                                    // Skip our own instance
                                    if daemon.instance_name == local_hostname {
                                        tracing::debug!(
                                            name = %daemon.instance_name,
                                            "mDNS: ignoring own instance"
                                        );
                                        continue;
                                    }
                                    tracing::info!(
                                        name = %daemon.instance_name,
                                        host = %daemon.host,
                                        addresses = ?daemon.addresses,
                                        port = daemon.port,
                                        has_auth = daemon.has_auth,
                                        "mDNS: discovered LAN daemon"
                                    );
                                    // Update existing or insert new
                                    if let Some(existing) = this
                                        .discovered_daemons
                                        .iter_mut()
                                        .find(|d| d.instance_name == daemon.instance_name)
                                    {
                                        if existing != &daemon {
                                            *existing = daemon;
                                            changed = true;
                                        }
                                    } else {
                                        this.discovered_daemons.push(daemon);
                                        changed = true;
                                    }
                                },
                                mdns_browser::MdnsEvent::Removed(name) => {
                                    tracing::info!(name = %name, "mDNS: LAN daemon removed");
                                    let before = this.discovered_daemons.len();
                                    this.discovered_daemons.retain(|d| d.instance_name != name);
                                    if this.discovered_daemons.len() != before {
                                        changed = true;
                                        // Rebuild remote_daemon_states with new indices
                                        let new_states: HashMap<usize, RemoteDaemonState> = this
                                            .remote_daemon_states
                                            .drain()
                                            .filter(|(idx, _)| *idx < this.discovered_daemons.len())
                                            .collect();
                                        this.remote_daemon_states = new_states;
                                        if let Some(idx) = this.active_discovered_daemon
                                            && idx >= this.discovered_daemons.len()
                                        {
                                            this.active_discovered_daemon = None;
                                        }
                                    }
                                },
                            }
                        }
                        if changed {
                            cx.set_menus(build_app_menus(&this.discovered_daemons));
                            cx.notify();
                        }
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn start_memory_poller(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (bridge_tx, bridge_rx) = smol::channel::bounded::<ProcessUsageSnapshot>(1);

            cx.background_spawn(async move {
                let mut sampler = ProcessUsageSampler::new();

                loop {
                    std::thread::sleep(MEMORY_POLLER_INTERVAL);

                    let snapshot = sampler
                        .as_mut()
                        .map(ProcessUsageSampler::snapshot)
                        .unwrap_or_default();

                    if bridge_tx.send(snapshot).await.is_err() {
                        break;
                    }
                }
            })
            .detach();

            loop {
                let Ok(snapshot) = bridge_rx.recv().await else {
                    break;
                };

                let updated = this.update(cx, |this, cx| {
                    let cpu_changed = this.self_cpu_percent != snapshot.cpu_percent;
                    let memory_changed = this.self_memory_bytes != snapshot.memory_bytes;

                    if cpu_changed {
                        this.self_cpu_percent = snapshot.cpu_percent;
                    }
                    if memory_changed {
                        this.self_memory_bytes = snapshot.memory_bytes;
                    }
                    if cpu_changed || memory_changed {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}

fn wait_for_terminal_poller_event(poll_rx: &Receiver<()>, wait_interval: Option<Duration>) -> bool {
    enum TerminalPollWait {
        TimedOut,
        Notified,
        Disconnected,
    }

    let wait_result = match wait_interval {
        Some(wait_interval) => match poll_rx.recv_timeout(wait_interval) {
            Ok(()) => TerminalPollWait::Notified,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => TerminalPollWait::TimedOut,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => TerminalPollWait::Disconnected,
        },
        None => match poll_rx.recv() {
            Ok(()) => TerminalPollWait::Notified,
            Err(_) => TerminalPollWait::Disconnected,
        },
    };

    if matches!(wait_result, TerminalPollWait::Disconnected) {
        return false;
    }

    while poll_rx.try_recv().is_ok() {}
    if matches!(wait_result, TerminalPollWait::Notified) {
        std::thread::sleep(Duration::from_millis(4));
        while poll_rx.try_recv().is_ok() {}
    }

    true
}
