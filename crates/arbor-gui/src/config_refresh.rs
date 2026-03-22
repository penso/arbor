use super::*;

impl ArborWindow {
    pub(crate) fn refresh_config_if_changed(&mut self, cx: &mut Context<Self>) {
        struct ConfigRefreshOutcome {
            next_modified: Option<SystemTime>,
            next_theme_kind: Option<ThemeKind>,
            next_backend_kind: Option<TerminalBackendKind>,
            next_embedded_shell: Option<String>,
            next_daemon_base_url: String,
            next_terminal_daemon: Option<terminal_daemon_http::SharedTerminalDaemonClient>,
            daemon_records: Option<Vec<DaemonSessionRecord>>,
            daemon_connection_refused: bool,
            remote_hosts: Vec<arbor_core::outpost::RemoteHost>,
            agent_presets: Vec<AgentPreset>,
            configured_providers: Vec<ConfiguredProvider>,
            notifications_enabled: bool,
            notices: Vec<String>,
        }

        let store = self.app_config_store.clone();
        let current_modified = self.config_last_modified;
        let current_daemon = self.terminal_daemon.clone();
        let current_daemon_base_url = self.daemon_base_url.clone();
        let next_epoch = self.config_refresh_epoch.wrapping_add(1);
        self.config_refresh_epoch = next_epoch;
        self._config_refresh_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let next_modified = store.config_last_modified();
                    if next_modified == current_modified {
                        return None;
                    }

                    let loaded = store.load_or_create_config();
                    let mut notices = loaded.notices;
                    arbor_terminal_emulator::set_default_terminal_scrollback_lines(
                        arbor_terminal_emulator::sanitize_terminal_scrollback_lines(
                            loaded.config.terminal_scrollback_lines,
                        ),
                    );

                    let next_theme_kind = match parse_theme_kind(loaded.config.theme.as_deref()) {
                        Ok(theme_kind) => Some(theme_kind),
                        Err(error) => {
                            notices.push(error.to_string());
                            None
                        },
                    };

                    let next_backend_kind =
                        match parse_terminal_backend_kind(loaded.config.terminal_backend.as_deref())
                        {
                            Ok(backend_kind) => Some(backend_kind),
                            Err(error) => {
                                notices.push(error.to_string());
                                None
                            },
                        };

                    let _ = resolve_embedded_terminal_engine(
                        loaded.config.embedded_terminal_engine.as_deref(),
                        &mut notices,
                    );

                    let next_daemon_base_url =
                        daemon_base_url_from_config(loaded.config.daemon_url.as_deref());
                    let daemon_url_changed = next_daemon_base_url != current_daemon_base_url;
                    if daemon_url_changed {
                        remove_claude_code_hooks();
                        remove_pi_agent_extension();
                    }

                    let next_terminal_daemon = if daemon_url_changed {
                        match terminal_daemon_http::default_terminal_daemon_client(
                            &next_daemon_base_url,
                        ) {
                            Ok(client) => Some(client),
                            Err(error) => {
                                notices.push(format!(
                                    "invalid daemon_url `{next_daemon_base_url}`: {error}"
                                ));
                                None
                            },
                        }
                    } else {
                        current_daemon.clone()
                    };

                    let mut daemon_records = None;
                    let mut daemon_connection_refused = false;
                    if let Some(daemon) = next_terminal_daemon.as_ref() {
                        match daemon.list_sessions() {
                            Ok(records) => daemon_records = Some(records),
                            Err(error) => {
                                let error_text = error.to_string();
                                daemon_connection_refused =
                                    daemon_error_is_connection_refused(&error_text);
                                if daemon_connection_refused {
                                    remove_claude_code_hooks();
                                    remove_pi_agent_extension();
                                }
                                if !daemon_connection_refused {
                                    notices.push(format!(
                                        "failed to list terminal sessions from daemon at {}: {error}",
                                        daemon.base_url()
                                    ));
                                }
                            },
                        }
                    }

                    let remote_hosts: Vec<arbor_core::outpost::RemoteHost> = loaded
                        .config
                        .remote_hosts
                        .iter()
                        .map(|host_config| arbor_core::outpost::RemoteHost {
                            name: host_config.name.clone(),
                            hostname: host_config.hostname.clone(),
                            port: host_config.port,
                            user: host_config.user.clone(),
                            identity_file: host_config.identity_file.clone(),
                            remote_base_path: host_config.remote_base_path.clone(),
                            daemon_port: host_config.daemon_port,
                            mosh: host_config.mosh,
                            mosh_server_path: host_config.mosh_server_path.clone(),
                        })
                        .collect();

                    Some(ConfigRefreshOutcome {
                        next_modified,
                        next_theme_kind,
                        next_backend_kind,
                        next_embedded_shell: loaded.config.embedded_shell.clone(),
                        next_daemon_base_url,
                        next_terminal_daemon,
                        daemon_records,
                        daemon_connection_refused,
                        remote_hosts,
                        agent_presets: normalize_agent_presets(&loaded.config.agent_presets),
                        configured_providers: load_configured_providers(
                            &loaded.config.providers,
                        ),
                        notifications_enabled: loaded.config.notifications.unwrap_or(true),
                        notices,
                    })
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.config_refresh_epoch != next_epoch {
                    return;
                }
                let Some(outcome) = outcome else {
                    return;
                };

                this.config_last_modified = outcome.next_modified;
                let mut changed = false;

                if let Some(theme_kind) = outcome.next_theme_kind
                    && this.theme_kind != theme_kind
                {
                    this.theme_kind = theme_kind;
                    changed = true;
                }
                if let Some(backend_kind) = outcome.next_backend_kind
                    && this.active_backend_kind != backend_kind
                {
                    this.active_backend_kind = backend_kind;
                    changed = true;
                }
                if this.configured_embedded_shell != outcome.next_embedded_shell {
                    this.configured_embedded_shell = outcome.next_embedded_shell.clone();
                    changed = true;
                }
                if this.daemon_base_url != outcome.next_daemon_base_url {
                    this.daemon_base_url = outcome.next_daemon_base_url.clone();
                    changed = true;
                }

                if outcome.daemon_connection_refused {
                    this.terminal_daemon = None;
                    changed = true;
                } else if this.terminal_daemon.as_ref().map(|daemon| daemon.base_url())
                    != outcome
                        .next_terminal_daemon
                        .as_ref()
                        .map(|daemon| daemon.base_url())
                {
                    this.terminal_daemon = outcome.next_terminal_daemon.clone();
                    changed = true;
                } else {
                    this.terminal_daemon = outcome.next_terminal_daemon.clone();
                }

                if let Some(records) = outcome.daemon_records {
                    changed |= this.restore_terminal_sessions_from_records(records, true);
                }

                if this.remote_hosts != outcome.remote_hosts {
                    this.remote_hosts = outcome.remote_hosts;
                    this.outposts =
                        load_outpost_summaries(this.outpost_store.as_ref(), &this.remote_hosts);
                    changed = true;
                }

                if this.agent_presets != outcome.agent_presets {
                    this.agent_presets = outcome.agent_presets;
                    if let Some(modal) = this.manage_presets_modal.as_mut()
                        && let Some(preset) = this
                            .agent_presets
                            .iter()
                            .find(|preset| preset.kind == modal.active_preset)
                    {
                        modal.command = preset.command.clone();
                    }
                    changed = true;
                }

                // Refresh configured providers from config.toml [[providers]]
                this.configured_providers = outcome.configured_providers;
                this.probe_provider_models(cx);

                if this.notifications_enabled != outcome.notifications_enabled {
                    this.notifications_enabled = outcome.notifications_enabled;
                    changed = true;
                }

                if !outcome.notices.is_empty() {
                    this.notice = Some(outcome.notices.join(" | "));
                    changed = true;
                }

                if changed {
                    cx.notify();
                }
            });
        }));
    }

    pub(crate) fn refresh_repo_config_if_changed(&mut self, cx: &mut Context<Self>) {
        let repo_root = self.repo_root.clone();
        let result_repo_root = repo_root.clone();
        let selected_worktree_path = self.selected_worktree_path().map(Path::to_path_buf);
        let repositories = self.repositories.clone();
        let store = self.app_config_store.clone();
        let next_epoch = self.repo_metadata_refresh_epoch.wrapping_add(1);
        self.repo_metadata_refresh_epoch = next_epoch;
        self._repo_metadata_refresh_task = Some(cx.spawn(async move |this, cx| {
            let (next_presets, next_default_preset, task_templates) = cx
                .background_spawn(async move {
                    let mut presets = load_repo_presets(store.as_ref(), &repo_root);
                    if let Some(worktree_path) = selected_worktree_path
                        .as_ref()
                        .filter(|worktree_path| *worktree_path != &repo_root)
                    {
                        for preset in load_repo_presets(store.as_ref(), worktree_path) {
                            if !presets
                                .iter()
                                .any(|candidate| candidate.name == preset.name)
                            {
                                presets.push(preset);
                            }
                        }
                    }
                    let default_preset = store
                        .load_repo_config(&repo_root)
                        .and_then(|config| config.agent.default_preset)
                        .and_then(|value| AgentPresetKind::from_key(&value));
                    let mut task_templates = Vec::new();
                    for repository in repositories {
                        task_templates.extend(load_task_templates_for_repo(&repository.root));
                    }
                    (presets, default_preset, task_templates)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.repo_metadata_refresh_epoch != next_epoch
                    || this.repo_root != result_repo_root
                {
                    return;
                }

                let mut changed = false;
                if this.repo_presets != next_presets {
                    this.repo_presets = next_presets;
                    changed = true;
                }
                if this.command_palette_task_templates != task_templates {
                    this.command_palette_task_templates = task_templates;
                    changed = true;
                }
                if this.active_preset_tab.is_none()
                    && let Some(preset) = next_default_preset
                {
                    this.active_preset_tab = Some(preset);
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            });
        }));
    }
}

pub(crate) fn parse_terminal_backend_kind(
    terminal_backend: Option<&str>,
) -> Result<TerminalBackendKind, ConfigParseError> {
    let Some(value) = terminal_backend
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(TerminalBackendKind::Embedded);
    };

    match value.to_ascii_lowercase().as_str() {
        "embedded" => Ok(TerminalBackendKind::Embedded),
        "alacritty" | "ghostty" => Err(ConfigParseError::InvalidValue(format!(
            "terminal_backend `{value}` is no longer supported; Arbor terminals are embedded-only. Using the embedded terminal instead. Configure `embedded_terminal_engine` to choose `alacritty` or `ghostty-vt-experimental`."
        ))),
        _ => Err(ConfigParseError::InvalidValue(format!(
            "invalid terminal_backend `{value}` in config, expected `embedded`"
        ))),
    }
}

pub(crate) fn parse_theme_kind(theme: Option<&str>) -> Result<ThemeKind, ConfigParseError> {
    let Some(value) = theme.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(ThemeKind::One);
    };

    ThemeKind::from_slug(value).ok_or_else(|| {
        let slugs: Vec<&str> = ThemeKind::ALL.iter().map(|k| k.slug()).collect();
        ConfigParseError::InvalidValue(format!(
            "invalid theme `{value}` in config, expected {}",
            slugs.join("/")
        ))
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use crate::{
        config_refresh::parse_terminal_backend_kind, terminal_backend::TerminalBackendKind,
        theme::ThemeKind,
    };

    #[test]
    fn parse_terminal_backend_defaults_to_embedded() {
        assert_eq!(
            parse_terminal_backend_kind(None),
            Ok(TerminalBackendKind::Embedded),
        );
        assert_eq!(
            parse_terminal_backend_kind(Some("")),
            Ok(TerminalBackendKind::Embedded),
        );
    }

    #[test]
    fn parse_terminal_backend_rejects_external_backends() {
        let alacritty = parse_terminal_backend_kind(Some("alacritty"));
        let ghostty = parse_terminal_backend_kind(Some("ghostty"));

        assert!(alacritty.is_err());
        assert!(ghostty.is_err());
    }

    #[test]
    fn parse_theme_kind_supports_solarized_light_aliases() {
        assert_eq!(
            crate::parse_theme_kind(Some("solarized-light")).ok(),
            Some(ThemeKind::SolarizedLight)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("solarized")).ok(),
            Some(ThemeKind::SolarizedLight)
        );
    }

    #[test]
    fn parse_theme_kind_supports_everforest_aliases() {
        assert_eq!(
            crate::parse_theme_kind(Some("everforest-dark")).ok(),
            Some(ThemeKind::Everforest)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("everforest")).ok(),
            Some(ThemeKind::Everforest)
        );
    }

    #[test]
    fn parse_theme_kind_supports_omarchy_and_custom_aliases() {
        assert_eq!(
            crate::parse_theme_kind(Some("catppuccin")).ok(),
            Some(ThemeKind::Catppuccin)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("catppuccin-latte")).ok(),
            Some(ThemeKind::CatppuccinLatte)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("ethereal")).ok(),
            Some(ThemeKind::Ethereal)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("flexoki-light")).ok(),
            Some(ThemeKind::FlexokiLight)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("hackerman")).ok(),
            Some(ThemeKind::Hackerman)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("kanagawa")).ok(),
            Some(ThemeKind::Kanagawa)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("matte-black")).ok(),
            Some(ThemeKind::MatteBlack)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("miasma")).ok(),
            Some(ThemeKind::Miasma)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("nord")).ok(),
            Some(ThemeKind::Nord)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("osaka-jade")).ok(),
            Some(ThemeKind::OsakaJade)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("ristretto")).ok(),
            Some(ThemeKind::Ristretto)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("rose-pine")).ok(),
            Some(ThemeKind::RosePine)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("tokyo-night")).ok(),
            Some(ThemeKind::TokyoNight)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("vantablack")).ok(),
            Some(ThemeKind::Vantablack)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("white")).ok(),
            Some(ThemeKind::White)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("atom-one-light")).ok(),
            Some(ThemeKind::AtomOneLight)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-light-default")).ok(),
            Some(ThemeKind::GitHubLightDefault)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-light-high-contrast")).ok(),
            Some(ThemeKind::GitHubLightHighContrast)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-light-colorblind")).ok(),
            Some(ThemeKind::GitHubLightColorblind)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-light")).ok(),
            Some(ThemeKind::GitHubLight)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-dark-default")).ok(),
            Some(ThemeKind::GitHubDarkDefault)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-dark-high-contrast")).ok(),
            Some(ThemeKind::GitHubDarkHighContrast)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-dark-colorblind")).ok(),
            Some(ThemeKind::GitHubDarkColorblind)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-dark-dimmed")).ok(),
            Some(ThemeKind::GitHubDarkDimmed)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-dark")).ok(),
            Some(ThemeKind::GitHubDark)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("retrobox-classic")).ok(),
            Some(ThemeKind::RetroboxClassic)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("retrobox")).ok(),
            Some(ThemeKind::RetroboxClassic)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("tokyonight-day")).ok(),
            Some(ThemeKind::TokyoNightDay)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("tokionight-day")).ok(),
            Some(ThemeKind::TokyoNightDay)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("tokyonight-classic")).ok(),
            Some(ThemeKind::TokyoNightClassic)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("tokionight-classic")).ok(),
            Some(ThemeKind::TokyoNightClassic)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("zellner")).ok(),
            Some(ThemeKind::Zellner)
        );
    }
}
