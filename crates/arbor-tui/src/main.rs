mod app;
mod capture;
mod client;
mod event;
mod header;
mod hooks;
mod tabs;
mod widgets;

use {
    app::App,
    clap::Parser,
    crossterm::{
        event::KeyEventKind,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
    hooks::BuiltinAction,
    ratatui::prelude::*,
    std::io,
};

#[derive(Parser)]
#[command(name = "arbor-tui", about = "Terminal dashboard for Arbor")]
struct Args {
    /// Daemon port
    #[arg(long, default_value = "8787")]
    port: u16,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));

    enable_raw_mode()?;
    crossterm::execute!(io::stdout(), EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let events = event::EventHandler::new(app.config.tui.tick_rate);
    let poller = client::DaemonPoller::start(args.port, app.config.tui.poll_interval);

    while app.running {
        terminal.draw(|frame| ui(frame, &mut app))?;

        match events.next() {
            Ok(event::Event::Key(key)) if key.kind == KeyEventKind::Press => {
                if app.input_mode {
                    match key.code {
                        crossterm::event::KeyCode::Esc => {
                            app.input_mode = false;
                            app.input_buffer.clear();
                        },
                        crossterm::event::KeyCode::Enter => {
                            if let Some(agent) = app.agents.get(app.agents_state.selected)
                                && let Some(backend) = capture::capture_for(agent)
                            {
                                backend.send_keys(agent, &app.input_buffer);
                            }
                            app.input_buffer.clear();
                        },
                        crossterm::event::KeyCode::Backspace => {
                            app.input_buffer.pop();
                        },
                        crossterm::event::KeyCode::Char(c) => {
                            app.input_buffer.push(c);
                        },
                        _ => {},
                    }
                } else if let Some(action) = app.config.lookup_builtin(key.code, key.modifiers) {
                    match action {
                        BuiltinAction::Quit => app.quit(),
                        BuiltinAction::NavDown => app.current_list_state_mut().select_next(),
                        BuiltinAction::NavUp => app.current_list_state_mut().select_prev(),
                        BuiltinAction::Refresh => {},
                        BuiltinAction::ToggleTable => {
                            app.table_collapsed = !app.table_collapsed;
                        },
                        BuiltinAction::ToggleMeta => {
                            app.meta_collapsed = !app.meta_collapsed;
                        },
                        BuiltinAction::ToggleHelp => {
                            app.show_help = !app.show_help;
                        },
                        BuiltinAction::ShowDetail => {
                            app.show_detail = !app.show_detail;
                        },
                        BuiltinAction::EnterInput => {
                            if app.pane_output.is_some() {
                                app.input_mode = true;
                            }
                        },
                    }
                } else if let crossterm::event::KeyCode::Char(c) = key.code {
                    let tab = app.current_action_tab();
                    if let Some(action_hook) = app.config.find_action(c, &tab) {
                        let env_vars = app.selected_env_vars();
                        let env_refs: Vec<(&str, &str)> =
                            env_vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
                        hooks::run_command(&action_hook.command, &env_refs);
                    }
                }
            },
            Ok(_) => {},
            Err(_) => {
                app.quit();
            },
        }

        let data = poller.drain();
        if !data.is_empty() {
            app.apply_daemon_data(data);
        }

        {
            if let Some(agent) = app.agents.get(app.agents_state.selected) {
                if agent
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("terminal"))
                    .is_some()
                {
                    poller.request_capture(agent);
                } else {
                    poller.clear_capture();
                    app.pane_output = None;
                }
            } else {
                poller.clear_capture();
                app.pane_output = None;
            }
        }
    }

    restore_terminal()?;
    Ok(())
}

fn restore_terminal() -> anyhow::Result<()> {
    disable_raw_mode()?;
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn ui(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(frame.area());

    let props = tabs::agents::AgentsTabProps {
        state: &app.agents_state,
        agents: &app.agents,
        pane_output: app.pane_output.as_deref(),
        input_text: if app.input_mode {
            Some(app.input_buffer.as_str())
        } else {
            None
        },
        header_segments: &app.config.tui.agent_header,
        field_colors: &app.config.tui.field_colors,
        status_icons: &app.config.tui.status_icons,
        column_header_color: app.config.tui.column_header_color,
        hidden_columns: &app.config.tui.hidden_columns,
        table_collapsed: app.table_collapsed,
        meta_collapsed: app.meta_collapsed,
        show_help: app.show_help,
        show_detail: app.show_detail,
    };
    tabs::agents::render_agents_tab(frame, chunks[0], &props);

    let action_tab = app.current_action_tab();
    let action_hints = app.config.action_hints(&action_tab);
    widgets::status_bar::render_status_bar(
        frame,
        chunks[1],
        app.connected,
        app.last_poll_secs(),
        &action_hints,
    );
}
