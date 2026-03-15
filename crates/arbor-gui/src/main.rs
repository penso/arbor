mod actions;
mod agent_activity;
mod agent_chat;
mod agent_presets;
mod app_bootstrap;
mod app_config;
mod app_init;
mod assets;
mod background_pollers;
mod center_panel;
mod changes_pane;
mod checkout;
mod command_palette;
mod config_refresh;
mod connection_history;
mod constants;
mod daemon_connection_ui;
mod daemon_runtime;
mod diff_engine;
mod diff_view;
mod error;
mod external_launchers;
mod file_view;
mod git_actions;
mod github_auth_modal;
mod github_auth_store;
mod github_helpers;
mod github_oauth;
mod github_pr_refresh;
mod github_service;
mod graphql;
mod helpers;
mod issue_cache_store;
mod issue_details_modal;
mod key_handling;
mod log_layer;
mod log_view;
mod manage_hosts;
mod managed_processes;
mod mdns_browser;
mod notifications;
mod port_detection;
mod pr_summary_ui;
mod prompt_runner;
mod rendering;
mod repo_presets;
mod repository_store;
mod settings_ui;
mod sidebar;
mod simple_http_client;
mod terminal_backend;
mod terminal_daemon_http;
mod terminal_interaction;
mod terminal_keys;
mod terminal_rendering;
mod terminal_session;
mod theme;
mod theme_picker;
mod top_bar;
mod types;
mod ui_state_store;
mod ui_widgets;
mod version_check;
mod welcome_ui;
mod workspace_layout;
mod workspace_navigation;
mod worktree_lifecycle;
mod worktree_refresh;
mod worktree_summary;

pub(crate) use {
    actions::*, agent_activity::*, agent_presets::*, app_bootstrap::*, assets::*,
    config_refresh::*, constants::*, daemon_runtime::*, diff_engine::*, diff_view::*, error::*,
    external_launchers::*, file_view::*, git_actions::*, github_helpers::*, github_oauth::*,
    github_pr_refresh::*, helpers::*, issue_details_modal::*, managed_processes::*,
    port_detection::*, pr_summary_ui::*, prompt_runner::*, rendering::*, repo_presets::*,
    settings_ui::*, terminal_rendering::*, theme_picker::*, types::*, ui_widgets::*,
    workspace_layout::*, worktree_refresh::*,
};
use {
    arbor_core::{
        agent::AgentState,
        changes::{self, ChangeKind, ChangedFile},
        daemon::{
            self, CreateOrAttachRequest, DaemonSessionRecord, DetachRequest, KillRequest,
            ResizeRequest, SignalRequest, TerminalSessionState, TerminalSignal, WriteRequest,
        },
        process::{
            ProcessSource, managed_process_session_title,
            managed_process_source_and_name_from_title,
        },
        procfile, repo_config, worktree,
        worktree_scripts::{WorktreeScriptContext, WorktreeScriptPhase, run_worktree_scripts},
    },
    checkout::CheckoutKind,
    gix_diff::blob::v2::{
        Algorithm as DiffAlgorithm, Diff as BlobDiff, InternedInput as BlobInternedInput,
    },
    gpui::{
        Animation, AnimationExt, AnyElement, App, Application, Bounds, ClipboardItem, Context, Div,
        DragMoveEvent, ElementId, ElementInputHandler, EntityInputHandler, FocusHandle, FontWeight,
        KeyBinding, KeyDownEvent, Keystroke, Menu, MenuItem, MouseButton, MouseDownEvent,
        MouseMoveEvent, MouseUpEvent, PathPromptOptions, Pixels, ScrollHandle, ScrollStrategy,
        Stateful, SystemMenuType, TextRun, UTF16Selection, UniformListScrollHandle, Window,
        WindowBounds, WindowControlArea, WindowOptions, canvas, div, ease_in_out, fill, img, point,
        prelude::*, px, rgb, size, uniform_list,
    },
    ropey::Rope,
    std::{
        collections::{HashMap, HashSet},
        env, fs,
        net::TcpListener,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::{
            Arc, Mutex, OnceLock,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant, SystemTime},
    },
    syntect::{easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet},
    terminal_backend::{
        EMBEDDED_TERMINAL_DEFAULT_BG, EMBEDDED_TERMINAL_DEFAULT_FG, EmbeddedTerminal,
        TerminalBackendKind, TerminalCursor, TerminalLaunch, TerminalModes, TerminalStyledCell,
        TerminalStyledLine, TerminalStyledRun,
    },
    theme::{ThemeKind, ThemePalette},
};

fn main() {
    use app_bootstrap::*;

    let program_name = env::args().next().unwrap_or_else(|| "arbor".to_owned());
    let launch_mode = match parse_launch_mode(env::args().skip(1)) {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("{error}\n\n{}", daemon_cli_usage(&program_name));
            std::process::exit(2);
        },
    };

    if matches!(launch_mode, LaunchMode::Help) {
        println!("{}", daemon_cli_usage(&program_name));
        return;
    }

    augment_path_from_login_shell();

    if let LaunchMode::Daemon { bind_addr } = launch_mode {
        if let Err(error) = run_daemon_mode(bind_addr) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }

    let log_buffer = log_layer::LogBuffer::new();

    {
        use tracing_subscriber::{
            EnvFilter, Layer, Registry, layer::SubscriberExt, util::SubscriberInitExt,
        };

        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let in_memory_layer =
            log_layer::InMemoryLayer::new(log_buffer.clone()).with_filter(env_filter);

        Registry::default().with(in_memory_layer).init();
    }

    tracing::info!("Arbor starting");

    run_gui(log_buffer);
}
