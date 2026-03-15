use {
    super::*,
    std::{collections::HashMap, time::Duration},
};

impl ArborWindow {
    pub(crate) fn start_terminal_poller(&mut self, cx: &mut Context<Self>) {
        let Some(poll_rx) = self.terminal_poll_rx.take() else {
            return;
        };

        cx.spawn(async move |this, cx| {
            let (bridge_tx, bridge_rx) = smol::channel::bounded::<()>(1);

            cx.background_spawn(async move {
                loop {
                    // Wait for a notification or fall back to 45ms timeout (for SSH/daemon
                    // terminals that use pull-based polling without a reader thread).
                    let _ = poll_rx.recv_timeout(Duration::from_millis(45));
                    // Drain queued notifications to coalesce burst output.
                    while poll_rx.try_recv().is_ok() {}
                    // Small deadline window to batch rapid output (e.g. `cat large_file`).
                    std::thread::sleep(Duration::from_millis(4));
                    while poll_rx.try_recv().is_ok() {}
                    if bridge_tx.send(()).await.is_err() {
                        break;
                    }
                }
            })
            .detach();

            loop {
                if bridge_rx.recv().await.is_err() {
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
}
