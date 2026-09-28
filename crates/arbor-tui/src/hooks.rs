use {
    crate::header::{self, FieldColor, Segment},
    crossterm::event::{KeyCode, KeyModifiers},
    ratatui::prelude::Color,
    std::{collections::HashMap, path::PathBuf, process::Command, time::Duration},
};

const DEFAULT_CONFIG: &str = include_str!("../config/default.toml");

#[derive(Debug, Clone)]
pub struct Config {
    pub tui: TuiSettings,
    pub keys: KeyBindings,
    pub actions: Vec<ActionHook>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusIcons {
    pub working: String,
    pub idle: String,
    pub other: String,
}

impl Default for StatusIcons {
    fn default() -> Self {
        Self {
            working: "●".to_owned(),
            idle: "○".to_owned(),
            other: "◌".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TuiSettings {
    pub tick_rate: Duration,
    pub poll_interval: Duration,
    pub agent_header: Vec<Segment>,
    pub field_colors: HashMap<String, FieldColor>,
    pub status_icons: StatusIcons,
    pub column_header_color: Color,
    pub hidden_columns: Vec<String>,
}

impl Default for TuiSettings {
    fn default() -> Self {
        Self {
            tick_rate: Duration::from_millis(250),
            poll_interval: Duration::from_millis(2000),
            agent_header: header::parse_format(
                "${session_id:8} ${cwd:-20} ${status:-10} ${elapsed:>8}",
            ),
            field_colors: HashMap::new(),
            status_icons: StatusIcons::default(),
            column_header_color: Color::White,
            hidden_columns: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyBindings(Vec<(KeySpec, BuiltinAction)>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySpec {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinAction {
    Quit,
    NavDown,
    NavUp,
    Refresh,
    ToggleTable,
    ToggleMeta,
    ToggleHelp,
    ShowDetail,
    EnterInput,
}

#[derive(Debug, Clone)]
pub struct ActionHook {
    pub name: String,
    pub key: char,
    pub command: String,
    pub tab: Option<ActionTab>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionTab {
    Agents,
    Repos,
}

impl Config {
    pub fn load() -> Self {
        let default = parse_toml(DEFAULT_CONFIG);
        let user_path = config_path();
        if !user_path.exists() {
            return default;
        }
        match std::fs::read_to_string(&user_path) {
            Ok(content) => merge_user_config(default, &content),
            Err(_) => default,
        }
    }

    pub fn lookup_builtin(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<BuiltinAction> {
        self.keys.0.iter().find_map(|(spec, action)| {
            if spec.code == code && spec.modifiers == modifiers {
                Some(*action)
            } else {
                None
            }
        })
    }

    pub fn find_action(&self, key: char, tab: &ActionTab) -> Option<&ActionHook> {
        self.actions
            .iter()
            .find(|a| a.key == key && a.tab.as_ref().is_none_or(|t| t == tab))
    }

    pub fn action_hints(&self, tab: &ActionTab) -> Vec<(char, &str)> {
        self.actions
            .iter()
            .filter(|a| a.tab.as_ref().is_none_or(|t| t == tab))
            .map(|a| (a.key, a.name.as_str()))
            .collect()
    }
}

pub fn run_command(command: &str, env_vars: &[(&str, &str)]) {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    for (key, val) in env_vars {
        cmd.env(key, val);
    }
    if let Ok(mut child) = cmd.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let xdg_path = PathBuf::from(xdg).join("arbor").join("tui.toml");
        if xdg_path.exists() {
            return xdg_path;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    let xdg_default = PathBuf::from(&home)
        .join(".config")
        .join("arbor")
        .join("tui.toml");
    if xdg_default.exists() {
        return xdg_default;
    }
    PathBuf::from(home).join(".arbor").join("tui.toml")
}

fn merge_user_config(default: Config, user_content: &str) -> Config {
    let table = match user_content.parse::<toml::Table>() {
        Ok(t) => t,
        Err(_) => return default,
    };

    let user_keys = parse_keys(table.get("keys").and_then(|v| v.as_table()));
    let keys = if user_keys.0.is_empty() {
        default.keys
    } else {
        user_keys
    };

    let overrides = parse_tui_overrides(table.get("tui").and_then(|v| v.as_table()));
    let tui = apply_tui_overrides(default.tui, overrides);

    let mut actions = default.actions;
    actions.extend(parse_actions(
        table.get("actions").and_then(|v| v.as_table()),
    ));

    Config { tui, keys, actions }
}

struct ParsedTuiOverrides {
    tick_rate: Option<Duration>,
    poll_interval: Option<Duration>,
    agent_header: Option<Vec<Segment>>,
    field_colors: Option<HashMap<String, FieldColor>>,
    status_icons: Option<StatusIcons>,
    column_header_color: Option<Color>,
    hidden_columns: Option<Vec<String>>,
}

fn apply_tui_overrides(base: TuiSettings, overrides: ParsedTuiOverrides) -> TuiSettings {
    TuiSettings {
        tick_rate: overrides.tick_rate.unwrap_or(base.tick_rate),
        poll_interval: overrides.poll_interval.unwrap_or(base.poll_interval),
        agent_header: overrides.agent_header.unwrap_or(base.agent_header),
        field_colors: overrides.field_colors.unwrap_or(base.field_colors),
        status_icons: overrides.status_icons.unwrap_or(base.status_icons),
        column_header_color: overrides
            .column_header_color
            .unwrap_or(base.column_header_color),
        hidden_columns: overrides.hidden_columns.unwrap_or(base.hidden_columns),
    }
}

fn parse_toml(content: &str) -> Config {
    let table = match content.parse::<toml::Table>() {
        Ok(t) => t,
        Err(_) => {
            return Config {
                tui: TuiSettings::default(),
                keys: KeyBindings(Vec::new()),
                actions: Vec::new(),
            };
        },
    };

    let tui = parse_tui_settings(table.get("tui").and_then(|v| v.as_table()));
    let keys = parse_keys(table.get("keys").and_then(|v| v.as_table()));
    let actions = parse_actions(table.get("actions").and_then(|v| v.as_table()));

    Config { tui, keys, actions }
}

fn parse_ms(table: &toml::Table, key: &str) -> Option<Duration> {
    table
        .get(key)
        .and_then(|v| v.as_integer())
        .map(|v| Duration::from_millis(v.clamp(0, i64::from(u32::MAX)) as u64))
}

fn parse_tui_settings(table: Option<&toml::Table>) -> TuiSettings {
    let Some(table) = table else {
        return TuiSettings::default();
    };
    let d = TuiSettings::default();
    TuiSettings {
        tick_rate: parse_ms(table, "tick_rate_ms").unwrap_or(d.tick_rate),
        poll_interval: parse_ms(table, "poll_interval_ms").unwrap_or(d.poll_interval),
        agent_header: table
            .get("agent_header")
            .and_then(|v| v.as_str())
            .map(header::parse_format)
            .unwrap_or(d.agent_header),
        field_colors: parse_field_colors(table.get("field_colors").and_then(|v| v.as_table())),
        status_icons: parse_status_icons(table.get("status_icons").and_then(|v| v.as_table())),
        column_header_color: table
            .get("column_header_color")
            .and_then(|v| v.as_str())
            .and_then(header::parse_color)
            .unwrap_or(d.column_header_color),
        hidden_columns: table
            .get("hidden_columns")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn parse_tui_overrides(table: Option<&toml::Table>) -> ParsedTuiOverrides {
    let Some(table) = table else {
        return ParsedTuiOverrides {
            tick_rate: None,
            poll_interval: None,
            agent_header: None,
            field_colors: None,
            status_icons: None,
            column_header_color: None,
            hidden_columns: None,
        };
    };

    let field_colors = table
        .get("field_colors")
        .and_then(|v| v.as_table())
        .map(|t| parse_field_colors(Some(t)))
        .filter(|m| !m.is_empty());

    let status_icons_table = table.get("status_icons").and_then(|v| v.as_table());
    let status_icons = status_icons_table.map(|t| parse_status_icons(Some(t)));

    let hidden_columns = table
        .get("hidden_columns")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                .collect()
        });

    ParsedTuiOverrides {
        tick_rate: parse_ms(table, "tick_rate_ms"),
        poll_interval: parse_ms(table, "poll_interval_ms"),
        agent_header: table
            .get("agent_header")
            .and_then(|v| v.as_str())
            .map(header::parse_format),
        field_colors,
        status_icons,
        column_header_color: table
            .get("column_header_color")
            .and_then(|v| v.as_str())
            .and_then(header::parse_color),
        hidden_columns,
    }
}

fn parse_status_icons(table: Option<&toml::Table>) -> StatusIcons {
    let d = StatusIcons::default();
    let Some(table) = table else {
        return d;
    };
    let str_val = |key: &str, default: &str| -> String {
        table
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_owned()
    };
    StatusIcons {
        working: str_val("working", &d.working),
        idle: str_val("idle", &d.idle),
        other: str_val("other", &d.other),
    }
}

fn parse_field_colors(table: Option<&toml::Table>) -> HashMap<String, FieldColor> {
    let Some(table) = table else {
        return HashMap::new();
    };

    let mut colors = HashMap::new();
    for (field, value) in table {
        if let Some(color_name) = value.as_str() {
            if let Some(color) = header::parse_color(color_name) {
                colors.insert(field.clone(), FieldColor::Static(color));
            }
        } else if let Some(map) = value.as_table() {
            let mut color_map = HashMap::new();
            for (val, color_name) in map {
                if let Some(cn) = color_name.as_str()
                    && let Some(color) = header::parse_color(cn)
                {
                    color_map.insert(val.clone(), color);
                }
            }
            if !color_map.is_empty() {
                colors.insert(field.clone(), FieldColor::Map(color_map));
            }
        }
    }
    colors
}

fn parse_keys(table: Option<&toml::Table>) -> KeyBindings {
    let Some(table) = table else {
        return KeyBindings(Vec::new());
    };

    let binding_names: &[(&str, BuiltinAction)] = &[
        ("quit", BuiltinAction::Quit),
        ("quit_alt", BuiltinAction::Quit),
        ("nav_down", BuiltinAction::NavDown),
        ("nav_down_alt", BuiltinAction::NavDown),
        ("nav_up", BuiltinAction::NavUp),
        ("nav_up_alt", BuiltinAction::NavUp),
        ("refresh", BuiltinAction::Refresh),
        ("toggle_table", BuiltinAction::ToggleTable),
        ("toggle_meta", BuiltinAction::ToggleMeta),
        ("toggle_help", BuiltinAction::ToggleHelp),
        ("show_detail", BuiltinAction::ShowDetail),
        ("enter_input", BuiltinAction::EnterInput),
    ];

    let mut bindings = Vec::new();
    for (name, action) in binding_names {
        if let Some(key_str) = table.get(*name).and_then(|v| v.as_str())
            && let Some(spec) = parse_key_spec(key_str)
        {
            bindings.push((spec, *action));
        }
    }

    KeyBindings(bindings)
}

fn parse_key_spec(s: &str) -> Option<KeySpec> {
    if let Some(c) = s.strip_prefix("C-") {
        let ch = c.chars().next()?;
        return Some(KeySpec {
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::CONTROL,
        });
    }

    match s {
        "Tab" => Some(KeySpec {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
        }),
        "Enter" => Some(KeySpec {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
        }),
        "Esc" => Some(KeySpec {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
        }),
        "Up" => Some(KeySpec {
            code: KeyCode::Up,
            modifiers: KeyModifiers::NONE,
        }),
        "Down" => Some(KeySpec {
            code: KeyCode::Down,
            modifiers: KeyModifiers::NONE,
        }),
        "Left" => Some(KeySpec {
            code: KeyCode::Left,
            modifiers: KeyModifiers::NONE,
        }),
        "Right" => Some(KeySpec {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
        }),
        s if s.len() == 1 => {
            let ch = s.chars().next()?;
            Some(KeySpec {
                code: KeyCode::Char(ch),
                modifiers: KeyModifiers::NONE,
            })
        },
        _ => None,
    }
}

fn parse_actions(table: Option<&toml::Table>) -> Vec<ActionHook> {
    let Some(table) = table else {
        return Vec::new();
    };

    let mut actions = Vec::new();
    for (name, value) in table {
        let key = value
            .get("key")
            .and_then(|v| v.as_str())
            .and_then(|s| s.chars().next());
        let command = value
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());
        let tab = value
            .get("tab")
            .and_then(|v| v.as_str())
            .and_then(|s| match s {
                "agents" => Some(ActionTab::Agents),
                "repos" => Some(ActionTab::Repos),
                _ => None,
            });

        if let (Some(key), Some(command)) = (key, command) {
            actions.push(ActionHook {
                name: name.clone(),
                key,
                command,
                tab,
            });
        }
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_default_config_loads_all_keybindings() {
        let config = parse_toml(DEFAULT_CONFIG);
        assert_eq!(config.keys.0.len(), 12);
    }

    #[test]
    fn test_parse_default_config_loads_tui_settings() {
        let config = parse_toml(DEFAULT_CONFIG);
        assert_eq!(config.tui.tick_rate, Duration::from_millis(250));
        assert_eq!(config.tui.poll_interval, Duration::from_millis(2000));
    }

    #[test]
    fn test_parse_key_spec_single_char() -> Result<(), Box<dyn std::error::Error>> {
        let spec = parse_key_spec("q").ok_or("should parse")?;
        assert_eq!(spec.code, KeyCode::Char('q'));
        assert_eq!(spec.modifiers, KeyModifiers::NONE);
        Ok(())
    }

    #[test]
    fn test_parse_key_spec_ctrl_modifier() -> Result<(), Box<dyn std::error::Error>> {
        let spec = parse_key_spec("C-c").ok_or("should parse")?;
        assert_eq!(spec.code, KeyCode::Char('c'));
        assert_eq!(spec.modifiers, KeyModifiers::CONTROL);
        Ok(())
    }

    #[test]
    fn test_parse_key_spec_special_keys() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(parse_key_spec("Tab").ok_or("Tab")?.code, KeyCode::Tab);
        assert_eq!(parse_key_spec("Enter").ok_or("Enter")?.code, KeyCode::Enter);
        assert_eq!(parse_key_spec("Esc").ok_or("Esc")?.code, KeyCode::Esc);
        assert_eq!(parse_key_spec("Up").ok_or("Up")?.code, KeyCode::Up);
        assert_eq!(parse_key_spec("Down").ok_or("Down")?.code, KeyCode::Down);
        Ok(())
    }

    #[test]
    fn test_parse_key_spec_invalid_returns_none() {
        assert!(parse_key_spec("InvalidKey").is_none());
        assert!(parse_key_spec("").is_none());
    }

    #[test]
    fn test_lookup_builtin_finds_bound_key() {
        let config = parse_toml(DEFAULT_CONFIG);
        let action = config.lookup_builtin(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(action, Some(BuiltinAction::Quit));
    }

    #[test]
    fn test_lookup_builtin_returns_none_for_unbound_key() {
        let config = parse_toml(DEFAULT_CONFIG);
        let action = config.lookup_builtin(KeyCode::Char('z'), KeyModifiers::NONE);
        assert!(action.is_none());
    }

    #[test]
    fn test_merge_user_keys_override_defaults() {
        let default = parse_toml(DEFAULT_CONFIG);
        let merged = merge_user_config(default, "[keys]\nquit = \"x\"\n");
        let action = merged.lookup_builtin(KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(action, Some(BuiltinAction::Quit));
        assert!(
            merged
                .lookup_builtin(KeyCode::Char('q'), KeyModifiers::NONE)
                .is_none()
        );
    }

    #[test]
    fn test_merge_user_actions_are_additive() {
        let default = parse_toml(DEFAULT_CONFIG);
        let merged = merge_user_config(
            default,
            "[actions.test]\nkey = \"g\"\ncommand = \"echo hi\"\ntab = \"agents\"\n",
        );
        assert_eq!(merged.actions.len(), 1);
        assert_eq!(merged.actions[0].key, 'g');
        assert_eq!(merged.actions[0].tab, Some(ActionTab::Agents));
    }

    #[test]
    fn test_merge_user_tui_settings_override() {
        let default = parse_toml(DEFAULT_CONFIG);
        let merged = merge_user_config(default, "[tui]\npoll_interval_ms = 5000\n");
        assert_eq!(merged.tui.poll_interval, Duration::from_millis(5000));
        assert_eq!(merged.tui.tick_rate, Duration::from_millis(250));
    }

    #[test]
    fn test_find_action_matches_tab_filter() {
        let config =
            parse_toml("[actions.go]\nkey = \"g\"\ncommand = \"echo\"\ntab = \"agents\"\n");
        assert!(config.find_action('g', &ActionTab::Agents).is_some());
        assert!(config.find_action('g', &ActionTab::Repos).is_none());
    }

    #[test]
    fn test_find_action_global_matches_any_tab() {
        let config = parse_toml("[actions.go]\nkey = \"g\"\ncommand = \"echo\"\n");
        assert!(config.find_action('g', &ActionTab::Agents).is_some());
        assert!(config.find_action('g', &ActionTab::Repos).is_some());
    }

    #[test]
    fn test_action_hints_filters_by_tab() {
        let config = parse_toml(
            "[actions.go]\nkey = \"g\"\ncommand = \"echo\"\ntab = \"agents\"\n\n[actions.open]\nkey = \"o\"\ncommand = \"open\"\n",
        );
        let hints = config.action_hints(&ActionTab::Agents);
        assert_eq!(hints.len(), 2);
        let hints = config.action_hints(&ActionTab::Repos);
        assert_eq!(hints.len(), 1);
    }

    #[test]
    fn test_invalid_toml_returns_empty_config() {
        let config = parse_toml("this is not valid toml {{{");
        assert!(config.keys.0.is_empty());
        assert!(config.actions.is_empty());
    }
}
