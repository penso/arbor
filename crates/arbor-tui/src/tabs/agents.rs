use {
    crate::{
        header::{FieldColor, Segment, resolve_color, resolve_field},
        hooks::StatusIcons,
        widgets::list_detail::ListDetailState,
    },
    ansi_to_tui::IntoText,
    arbor_daemon_client::AgentSessionDto,
    ratatui::{
        prelude::*,
        widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
    },
    std::collections::HashMap,
};

const STATUS_WORKING: &str = "working";
const STATUS_IDLE: &str = "idle";
const TABLE_TITLE: &str = "Agents";
const META_TITLE: &str = "Metadata";
const PANE_TITLE: &str = "Terminal (i=input)";
const INPUT_BAR_TITLE: &str = " Send to pane (Esc to cancel) ";
const EMPTY_STATE_MSG: &str =
    "No agents detected.\n\nAgents appear when Claude Code hooks\nPOST to /api/v1/agent/notify.";
const NO_META_MSG: &str = "This agent is not publishing metadata via hooks.";
const MAX_TABLE_ROWS: u16 = 12;
const TABLE_CHROME: u16 = 3;
const COLLAPSED_HEIGHT: u16 = 1;
const META_ROWS: u16 = 6;
const META_CHROME: u16 = 2;
const LABEL_WIDTH: usize = 16;
const ICON_COL_WIDTH: u16 = 2;
const AUTO_COLUMN_WIDTH: u16 = 10;
const HELP_POPUP_WIDTH: u16 = 44;
const DETAIL_WIDTH_PCT: u16 = 90;
const DETAIL_HEIGHT_PCT: u16 = 80;
const INPUT_BAR_HEIGHT: u16 = 3;
const MIN_PANE_HEIGHT: u16 = 4;
const DEFAULT_FIELD_WIDTH: u16 = 12;
const HIDDEN_META_KEYS: &[&str] = &["terminal"];

const HELP_TEXT: &str = "\
Keybindings:
  q / C-c    Quit
  j / Down   Navigate down
  k / Up     Navigate up
  t          Toggle agents table
  m          Toggle metadata panel
  Enter      Agent detail popup
  r          Refresh
  i          Send input to terminal pane
  ?          Toggle this help";

pub struct AgentsTabProps<'a> {
    pub state: &'a ListDetailState,
    pub agents: &'a [AgentSessionDto],
    pub pane_output: Option<&'a str>,
    pub input_text: Option<&'a str>,
    pub header_segments: &'a [Segment],
    pub field_colors: &'a HashMap<String, FieldColor>,
    pub status_icons: &'a StatusIcons,
    pub column_header_color: Color,
    pub hidden_columns: &'a [String],
    pub table_collapsed: bool,
    pub meta_collapsed: bool,
    pub show_help: bool,
    pub show_detail: bool,
}

struct TableColumn {
    name: String,
    width: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetaState {
    None,
    HiddenOnly,
    HasFields,
}

fn is_visible_meta_key(key: &str) -> bool {
    !HIDDEN_META_KEYS.contains(&key)
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

fn label_line(label: &str, value: &str, label_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{:<LABEL_WIDTH$} ", label), label_style),
        Span::raw(value.to_owned()),
    ])
}

fn styled_label_line(
    label: &str,
    value: &str,
    label_style: Style,
    value_style: Style,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{:<LABEL_WIDTH$} ", label), label_style),
        Span::styled(value.to_owned(), value_style),
    ])
}

fn format_scalar(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Recursively sort object keys in a JSON value so output is deterministic
/// regardless of whether `serde_json/preserve_order` is enabled.
fn sort_json_keys(val: &serde_json::Value) -> serde_json::Value {
    match val {
        serde_json::Value::Object(map) => {
            let mut sorted: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                if let Some(v) = map.get(k) {
                    sorted.insert(k.clone(), sort_json_keys(v));
                }
            }
            serde_json::Value::Object(sorted)
        },
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_json_keys).collect())
        },
        other => other.clone(),
    }
}

fn classify_metadata(agent: Option<&AgentSessionDto>) -> MetaState {
    match agent
        .and_then(|a| a.metadata.as_ref())
        .and_then(|m| m.as_object())
    {
        None => MetaState::None,
        Some(obj) if obj.keys().any(|k| is_visible_meta_key(k)) => MetaState::HasFields,
        Some(_) => MetaState::HiddenOnly,
    }
}

fn status_icon<'a>(state: &str, icons: &'a StatusIcons) -> &'a str {
    match state {
        STATUS_WORKING => icons.working.as_str(),
        STATUS_IDLE => icons.idle.as_str(),
        _ => icons.other.as_str(),
    }
}

fn status_color(state: &str) -> Color {
    match state {
        STATUS_WORKING => Color::Green,
        STATUS_IDLE => Color::DarkGray,
        _ => Color::Red,
    }
}

fn extract_columns(
    segments: &[Segment],
    agents: &[AgentSessionDto],
    hidden: &[String],
) -> Vec<TableColumn> {
    let mut columns: Vec<TableColumn> = segments
        .iter()
        .filter_map(|seg| match seg {
            Segment::Field { name, width } => {
                let w = width
                    .map(|w| w.unsigned_abs() as u16)
                    .unwrap_or(DEFAULT_FIELD_WIDTH);
                Some(TableColumn {
                    name: name.clone(),
                    width: w,
                })
            },
            Segment::Literal(_) => None,
        })
        .collect();

    let known: std::collections::HashSet<&str> = columns.iter().map(|c| c.name.as_str()).collect();
    let mut extra: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for agent in agents {
        if let Some(obj) = agent.metadata.as_ref().and_then(|m| m.as_object()) {
            for (key, val) in obj {
                if !known.contains(key.as_str())
                    && is_visible_meta_key(key)
                    && !hidden.iter().any(|h| h == key)
                    && !val.is_object()
                    && !val.is_array()
                {
                    extra.insert(key.clone());
                }
            }
        }
    }

    columns.extend(extra.into_iter().map(|name| TableColumn {
        name,
        width: AUTO_COLUMN_WIDTH,
    }));
    columns
}

fn build_layout(area: Rect, props: &AgentsTabProps<'_>, has_terminal: bool) -> std::rc::Rc<[Rect]> {
    let table_h = if props.table_collapsed {
        COLLAPSED_HEIGHT
    } else {
        (props.agents.len() as u16).min(MAX_TABLE_ROWS) + TABLE_CHROME
    };

    let meta_h = if props.meta_collapsed {
        COLLAPSED_HEIGHT
    } else if has_terminal {
        META_ROWS + META_CHROME
    } else {
        0
    };

    let mut constraints = vec![Constraint::Length(table_h)];

    if has_terminal {
        constraints.push(Constraint::Length(meta_h));
        constraints.push(Constraint::Min(MIN_PANE_HEIGHT));
    } else if props.meta_collapsed {
        constraints.push(Constraint::Length(COLLAPSED_HEIGHT));
    } else {
        constraints.push(Constraint::Min(MIN_PANE_HEIGHT));
    }

    if props.input_text.is_some() && has_terminal {
        constraints.push(Constraint::Length(INPUT_BAR_HEIGHT));
    }

    Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area)
}

fn render_collapsed_bar(frame: &mut Frame, area: Rect, title: &str, key: char) {
    let bar = Paragraph::new("").block(
        Block::default()
            .borders(Borders::TOP)
            .title(format!("{title} [{key} to expand]")),
    );
    frame.render_widget(bar, area);
}

pub fn render_agents_tab(frame: &mut Frame, area: Rect, props: &AgentsTabProps<'_>) {
    if props.agents.is_empty() {
        let msg = Paragraph::new(EMPTY_STATE_MSG)
            .block(Block::default().borders(Borders::ALL).title(TABLE_TITLE));
        frame.render_widget(msg, area);
        return;
    }

    let columns = extract_columns(props.header_segments, props.agents, props.hidden_columns);
    let selected_agent = props.agents.get(props.state.selected);
    let meta_state = classify_metadata(selected_agent);
    let has_terminal = props.pane_output.is_some();
    let chunks = build_layout(area, props, has_terminal);

    if props.table_collapsed {
        render_collapsed_bar(frame, chunks[0], TABLE_TITLE, 't');
    } else {
        render_agent_table(frame, chunks[0], props, &columns);
    }

    if props.meta_collapsed {
        render_collapsed_bar(frame, chunks[1], META_TITLE, 'm');
    } else {
        render_metadata_panel(
            frame,
            chunks[1],
            selected_agent,
            meta_state,
            props.field_colors,
        );
    }

    if has_terminal {
        render_terminal_pane(frame, chunks[2], props);
        if let Some(buf) = props.input_text {
            render_input_bar(frame, chunks[3], buf);
        }
    }

    if let Some(agent) = selected_agent.filter(|_| props.show_detail) {
        render_detail_overlay(frame, area, agent);
    }

    if props.show_help {
        render_help_overlay(frame, area);
    }
}

fn render_agent_table(
    frame: &mut Frame,
    area: Rect,
    props: &AgentsTabProps<'_>,
    columns: &[TableColumn],
) {
    let header_cells: Vec<Cell<'_>> =
        std::iter::once(Cell::from("").style(Style::default().bold()))
            .chain(columns.iter().map(|col| {
                Cell::from(col.name.clone())
                    .style(Style::default().bold().fg(props.column_header_color))
            }))
            .collect();

    let rows: Vec<Row<'_>> = props
        .agents
        .iter()
        .enumerate()
        .map(|(i, agent)| {
            let icon = status_icon(&agent.state, props.status_icons);
            let color = status_color(&agent.state);
            let mut cells: Vec<Cell<'_>> =
                vec![Cell::from(icon.to_owned()).style(Style::default().fg(color))];
            for col in columns {
                let value = resolve_field(agent, &col.name).unwrap_or_default();
                let style = resolve_color(props.field_colors, &col.name, &value)
                    .map(|c| Style::default().fg(c))
                    .unwrap_or_default();
                cells.push(Cell::from(value).style(style));
            }
            let row = Row::new(cells);
            if i == props.state.selected {
                row.style(Style::default().bg(Color::DarkGray).fg(Color::White))
            } else {
                row
            }
        })
        .collect();

    let widths: Vec<Constraint> = std::iter::once(Constraint::Length(ICON_COL_WIDTH))
        .chain(columns.iter().map(|col| Constraint::Min(col.width)))
        .collect();

    let table = Table::new(rows, &widths)
        .header(Row::new(header_cells).height(1))
        .block(Block::default().borders(Borders::ALL).title(TABLE_TITLE))
        .column_spacing(1);

    frame.render_widget(table, area);
}

fn render_metadata_panel(
    frame: &mut Frame,
    area: Rect,
    agent: Option<&AgentSessionDto>,
    meta_state: MetaState,
    field_colors: &HashMap<String, FieldColor>,
) {
    let block = Block::default().borders(Borders::ALL).title(META_TITLE);
    let label_style = Style::default().bold();

    let Some(agent) = agent else {
        frame.render_widget(Paragraph::new("No agent selected").block(block), area);
        return;
    };

    match meta_state {
        MetaState::None => {
            let warning = Paragraph::new(Line::from(Span::styled(
                NO_META_MSG,
                Style::default().fg(Color::Yellow).bold(),
            )))
            .block(block);
            frame.render_widget(warning, area);
        },
        MetaState::HiddenOnly => {
            let lines = vec![
                label_line("Session", &agent.session_id, label_style),
                label_line("CWD", &agent.cwd, label_style),
                label_line("State", &agent.state, label_style),
            ];
            frame.render_widget(Paragraph::new(lines).block(block), area);
        },
        MetaState::HasFields => {
            let lines = build_metadata_lines(agent, field_colors, label_style);
            frame.render_widget(Paragraph::new(lines).block(block), area);
        },
    }
}

fn build_metadata_lines(
    agent: &AgentSessionDto,
    field_colors: &HashMap<String, FieldColor>,
    label_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let Some(obj) = agent.metadata.as_ref().and_then(|m| m.as_object()) else {
        return lines;
    };
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for key in keys {
        let Some(val) = obj.get(key) else {
            continue;
        };
        if !is_visible_meta_key(key) {
            continue;
        }
        let Some(val_str) = format_scalar(val) else {
            continue;
        };
        let value_style = resolve_color(field_colors, key, &val_str)
            .map(|c| Style::default().fg(c))
            .unwrap_or_default();
        lines.push(styled_label_line(key, &val_str, label_style, value_style));
    }
    lines
}

fn render_terminal_pane(frame: &mut Frame, area: Rect, props: &AgentsTabProps<'_>) {
    let text = props.pane_output.unwrap_or("");
    let styled = text
        .as_bytes()
        .into_text()
        .unwrap_or_else(|_| Text::raw(text));
    let pane = Paragraph::new(styled)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(PANE_TITLE));
    frame.render_widget(pane, area);
}

fn render_input_bar(frame: &mut Frame, area: Rect, buf: &str) {
    let input = Paragraph::new(Line::from(vec![
        Span::styled("› ", Style::default().fg(Color::Yellow)),
        Span::raw(buf),
        Span::styled("█", Style::default().fg(Color::Yellow)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(INPUT_BAR_TITLE),
    );
    frame.render_widget(input, area);
}

fn render_detail_overlay(frame: &mut Frame, area: Rect, agent: &AgentSessionDto) {
    let label_style = Style::default().bold();
    let mut lines = vec![
        label_line("session_id", &agent.session_id, label_style),
        label_line("cwd", &agent.cwd, label_style),
        label_line("state", &agent.state, label_style),
        label_line(
            "updated_at",
            &agent.updated_at_unix_ms.to_string(),
            label_style,
        ),
    ];

    if let Some(obj) = agent.metadata.as_ref().and_then(|m| m.as_object()) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "── Metadata ──",
            Style::default().fg(Color::Yellow),
        )));
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        for key in keys {
            let Some(val) = obj.get(key) else {
                continue;
            };
            let val_str = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => "null".to_owned(),
                other => {
                    let sorted = sort_json_keys(other);
                    serde_json::to_string_pretty(&sorted).unwrap_or_default()
                },
            };
            for (i, line_str) in val_str.lines().enumerate() {
                let label = if i == 0 {
                    key.as_str()
                } else {
                    ""
                };
                lines.push(label_line(label, line_str, label_style));
            }
        }
    }

    let popup = centered_rect(
        area,
        area.width * DETAIL_WIDTH_PCT / 100,
        area.height * DETAIL_HEIGHT_PCT / 100,
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Agent Detail (Enter to close) "),
        ),
        popup,
    );
}

fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let height = (HELP_TEXT.lines().count() as u16) + 2;
    let popup = centered_rect(area, HELP_POPUP_WIDTH, height);

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(HELP_TEXT).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Help (? to close) "),
        ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::header,
        ratatui::{Terminal, backend::TestBackend},
    };

    const FIXED_TS: u64 = u64::MAX;

    fn make_agent(session_id: &str, cwd: &str, state: &str) -> AgentSessionDto {
        AgentSessionDto {
            session_id: session_id.to_owned(),
            cwd: cwd.to_owned(),
            state: state.to_owned(),
            updated_at_unix_ms: FIXED_TS,
            metadata: None,
        }
    }

    fn make_agent_with_meta(
        session_id: &str,
        cwd: &str,
        state: &str,
        meta: serde_json::Value,
    ) -> AgentSessionDto {
        AgentSessionDto {
            session_id: session_id.to_owned(),
            cwd: cwd.to_owned(),
            state: state.to_owned(),
            updated_at_unix_ms: FIXED_TS,
            metadata: Some(meta),
        }
    }

    fn render_to_string(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer().clone();
        let mut lines: Vec<String> = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    fn color_to_hex(c: Color) -> &'static str {
        match c {
            Color::Black => "#45475a",
            Color::Red => "#f38ba8",
            Color::Green => "#a6e3a1",
            Color::Yellow => "#f9e2af",
            Color::Blue => "#89b4fa",
            Color::Magenta => "#f5c2e7",
            Color::Cyan => "#89dceb",
            Color::Gray | Color::White => "#cdd6f4",
            Color::DarkGray => "#585b70",
            Color::LightRed => "#f38ba8",
            Color::LightGreen => "#a6e3a1",
            Color::LightYellow => "#f9e2af",
            Color::LightBlue => "#89b4fa",
            Color::LightMagenta => "#f5c2e7",
            Color::LightCyan => "#89dceb",
            _ => "#cdd6f4",
        }
    }

    fn html_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    fn render_to_svg(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer().clone();
        let w = buf.area.width;
        let h = buf.area.height;
        let char_w: f64 = 7.8;
        let char_h: f64 = 16.0;
        let pad: f64 = 12.0;
        let svg_w = (w as f64) * char_w + pad * 2.0;
        let svg_h = (h as f64) * char_h + pad * 2.0;

        let bg = "#1e1e2e";
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{svg_w}" height="{svg_h}"><style>text {{ font-family: 'JetBrains Mono','Fira Code','Cascadia Code','SF Mono',monospace; font-size: 13px; white-space: pre; }}</style><rect width="100%" height="100%" fill="{bg}"/>"#,
        );

        for y in 0..h {
            struct Run {
                text: String,
                fg: Color,
                bg: Color,
                bold: bool,
            }
            let mut runs: Vec<Run> = Vec::new();

            for x in 0..w {
                let cell = &buf[(x, y)];
                let fg = cell.fg;
                let bg = cell.bg;
                let bold = cell.modifier.contains(Modifier::BOLD);
                let sym = cell.symbol();

                if let Some(last) = runs.last_mut()
                    && last.fg == fg
                    && last.bg == bg
                    && last.bold == bold
                {
                    last.text.push_str(sym);
                    continue;
                }
                runs.push(Run {
                    text: sym.to_owned(),
                    fg,
                    bg,
                    bold,
                });
            }

            let mut x_offset: f64 = 0.0;
            for run in &runs {
                let run_w = (run.text.chars().count() as f64) * char_w;
                if run.bg != Color::Reset {
                    let bg_hex = color_to_hex(run.bg);
                    svg.push_str(&format!(
                        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                        pad + x_offset,
                        pad + (y as f64) * char_h,
                        run_w,
                        char_h,
                        bg_hex,
                    ));
                }
                x_offset += run_w;
            }

            let text_y = pad + (y as f64) * char_h + char_h * 0.75;
            svg.push_str(&format!(r#"<text y="{}">"#, text_y));
            let mut cx: f64 = pad;
            for run in &runs {
                let trimmed = run.text.as_str();
                if trimmed.is_empty() {
                    continue;
                }
                let fg_hex = color_to_hex(if run.fg == Color::Reset {
                    Color::Gray
                } else {
                    run.fg
                });
                let weight = if run.bold {
                    r#" font-weight="bold""#
                } else {
                    ""
                };
                svg.push_str(&format!(
                    r#"<tspan x="{}"{} fill="{}">{}</tspan>"#,
                    cx,
                    weight,
                    fg_hex,
                    html_escape(trimmed),
                ));
                cx += (run.text.chars().count() as f64) * char_w;
            }
            svg.push_str("</text>");
        }

        svg.push_str("</svg>");
        svg
    }

    fn save_screenshot(
        terminal: &Terminal<TestBackend>,
        name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let svg = render_to_svg(terminal);
        let screenshots_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("screenshots");
        std::fs::create_dir_all(&screenshots_dir)?;
        std::fs::write(screenshots_dir.join(format!("{name}.svg")), &svg)?;
        Ok(())
    }

    fn make_props<'a>(
        state: &'a ListDetailState,
        agents: &'a [AgentSessionDto],
        pane_output: Option<&'a str>,
        segments: &'a [Segment],
    ) -> AgentsTabProps<'a> {
        static EMPTY_COLORS: std::sync::LazyLock<HashMap<String, FieldColor>> =
            std::sync::LazyLock::new(HashMap::new);
        static DEFAULT_ICONS: std::sync::LazyLock<StatusIcons> =
            std::sync::LazyLock::new(StatusIcons::default);
        static EMPTY_HIDDEN: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(Vec::new);
        AgentsTabProps {
            state,
            agents,
            pane_output,
            input_text: None,
            header_segments: segments,
            field_colors: &EMPTY_COLORS,
            status_icons: &DEFAULT_ICONS,
            column_header_color: Color::White,
            hidden_columns: &EMPTY_HIDDEN,
            table_collapsed: false,
            meta_collapsed: false,
            show_help: false,
            show_detail: false,
        }
    }

    fn make_terminal<F>(
        width: u16,
        height: u16,
        agents: &[AgentSessionDto],
        segments: &[Segment],
        pane_output: Option<&str>,
        customize: F,
    ) -> Result<Terminal<TestBackend>, Box<dyn std::error::Error>>
    where
        F: FnOnce(&mut AgentsTabProps<'_>),
    {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend)?;
        let mut state = ListDetailState::new();
        state.set_count(agents.len());

        terminal.draw(|frame| {
            let mut props = make_props(&state, agents, pane_output, segments);
            customize(&mut props);
            render_agents_tab(frame, frame.area(), &props);
        })?;

        Ok(terminal)
    }

    fn draw_agents(
        width: u16,
        height: u16,
        agents: &[AgentSessionDto],
        segments: &[Segment],
        pane_output: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let terminal = make_terminal(width, height, agents, segments, pane_output, |_| {})?;
        Ok(render_to_string(&terminal))
    }

    fn draw_agents_with<F>(
        width: u16,
        height: u16,
        agents: &[AgentSessionDto],
        segments: &[Segment],
        pane_output: Option<&str>,
        customize: F,
    ) -> Result<String, Box<dyn std::error::Error>>
    where
        F: FnOnce(&mut AgentsTabProps<'_>),
    {
        let terminal = make_terminal(width, height, agents, segments, pane_output, customize)?;
        Ok(render_to_string(&terminal))
    }

    #[test]
    fn snapshot_empty_state() -> Result<(), Box<dyn std::error::Error>> {
        let output = draw_agents(80, 10, &[], &[], None)?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_single_agent_no_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${status:-10} ${elapsed:>8}");
        let agents = vec![make_agent("s1", "/home/user/myapp", "working")];
        let output = draw_agents(80, 20, &agents, &segments, None)?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_single_agent_with_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${project:-20} ${status:-10}");
        let agents = vec![make_agent_with_meta(
            "s1",
            "/home/user/myapp",
            "working",
            serde_json::json!({"project": "myapp", "branch": "main"}),
        )];
        let output = draw_agents(80, 20, &agents, &segments, None)?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_multiple_agents() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${project:-20} ${status:-10}");
        let agents = vec![
            make_agent_with_meta(
                "s1",
                "/a/alpha",
                "working",
                serde_json::json!({"project": "alpha"}),
            ),
            make_agent_with_meta(
                "s2",
                "/b/beta",
                "idle",
                serde_json::json!({"project": "beta"}),
            ),
        ];
        let output = draw_agents(80, 25, &agents, &segments, None)?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_with_terminal_pane() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${status:-10}");
        let agents = vec![make_agent("s1", "/home/user/project", "working")];
        let output = draw_agents(80, 25, &agents, &segments, Some("$ cargo test\nall passed"))?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_auto_discovered_columns() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${status:-10}");
        let agents = vec![make_agent_with_meta(
            "s1",
            "/a/myapp",
            "working",
            serde_json::json!({
                "ws_status": "idle",
                "blocked_on": "review",
                "terminal": {"type": "tmux", "server": "default", "pane_id": "%0"}
            }),
        )];
        let output = draw_agents(100, 20, &agents, &segments, None)?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_hidden_only_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${status:-10}");
        let agents = vec![make_agent_with_meta(
            "s1",
            "/home/user/myapp",
            "working",
            serde_json::json!({"terminal": {"type": "tmux", "server": "default", "pane_id": "%0"}}),
        )];
        let output = draw_agents(80, 20, &agents, &segments, Some("$ echo hello"))?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_detail_overlay_with_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${project:-20} ${status:-10}");
        let agents = vec![make_agent_with_meta(
            "s1",
            "/home/user/myapp",
            "working",
            serde_json::json!({
                "pid": "12345",
                "project": "myapp",
                "branch": "feat/new-api",
                "terminal": {"type": "tmux", "server": "default", "pane_id": "%3"}
            }),
        )];
        let output = draw_agents_with(80, 30, &agents, &segments, None, |props| {
            props.show_detail = true;
        })?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_detail_overlay_no_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${status:-10}");
        let agents = vec![make_agent("s1", "/home/user/project", "idle")];
        let output = draw_agents_with(80, 25, &agents, &segments, None, |props| {
            props.show_detail = true;
        })?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_table_collapsed() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${project:-20} ${status:-10}");
        let agents = vec![make_agent_with_meta(
            "s1",
            "/home/user/myapp",
            "working",
            serde_json::json!({"project": "myapp", "branch": "main"}),
        )];
        let output = draw_agents_with(80, 20, &agents, &segments, None, |props| {
            props.table_collapsed = true;
        })?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_meta_collapsed() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${project:-20} ${status:-10}");
        let agents = vec![make_agent_with_meta(
            "s1",
            "/home/user/myapp",
            "working",
            serde_json::json!({"project": "myapp"}),
        )];
        let output = draw_agents_with(80, 20, &agents, &segments, None, |props| {
            props.meta_collapsed = true;
        })?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_both_collapsed() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${status:-10}");
        let agents = vec![make_agent("s1", "/home/user/myapp", "working")];
        let output = draw_agents_with(
            80,
            20,
            &agents,
            &segments,
            Some("$ make build\nok"),
            |props| {
                props.table_collapsed = true;
                props.meta_collapsed = true;
            },
        )?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_help_overlay() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${status:-10}");
        let agents = vec![make_agent("s1", "/home/user/myapp", "working")];
        let output = draw_agents_with(80, 25, &agents, &segments, None, |props| {
            props.show_help = true;
        })?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    fn snapshot_input_bar() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format("${status:-10}");
        let agents = vec![make_agent("s1", "/home/user/project", "working")];
        let output = draw_agents_with(
            80,
            25,
            &agents,
            &segments,
            Some("$ cargo test\nrunning..."),
            |props| {
                props.input_text = Some("make build");
            },
        )?;
        insta::assert_snapshot!(output);
        Ok(())
    }

    #[test]
    #[ignore]
    fn generate_screenshots() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format(
            "${pid:6} ${session_id:8} ${cwd:-20} ${project:-12} ${branch:-12} ${status:-8} ${elapsed:>6}",
        );
        let agents = vec![
            make_agent_with_meta(
                "abc12345",
                "/home/user/frontend",
                "working",
                serde_json::json!({"pid": "9001", "project": "frontend", "branch": "main"}),
            ),
            make_agent_with_meta(
                "def67890",
                "/home/user/backend",
                "idle",
                serde_json::json!({"pid": "9002", "project": "backend", "branch": "feat/api"}),
            ),
            make_agent_with_meta(
                "ghi11111",
                "/home/user/infra",
                "working",
                serde_json::json!({"pid": "9003", "project": "infra", "branch": "fix/deploy", "blocked_on": "review"}),
            ),
        ];
        save_screenshot(
            &make_terminal(120, 20, &agents, &segments, None, |_| {})?,
            "main-view",
        )?;

        let segments = header::parse_format("${project:-20} ${status:-10}");
        let agents = vec![make_agent_with_meta(
            "s1",
            "/home/user/myapp",
            "working",
            serde_json::json!({
                "pid": "12345",
                "project": "myapp",
                "branch": "feat/new-api",
                "terminal": {"type": "tmux", "server": "default", "pane_id": "%3"}
            }),
        )];
        save_screenshot(
            &make_terminal(80, 28, &agents, &segments, None, |p| {
                p.show_detail = true;
            })?,
            "detail-overlay",
        )?;

        let segments = header::parse_format("${project:-16} ${status:-8}");
        let agents = vec![make_agent_with_meta(
            "s1",
            "/home/user/project",
            "working",
            serde_json::json!({
                "project": "myproject",
                "terminal": {"type": "tmux", "server": "default", "pane_id": "%1"}
            }),
        )];
        save_screenshot(
            &make_terminal(
                80,
                25,
                &agents,
                &segments,
                Some(
                    "$ cargo test\nrunning 12 tests\ntest parse ... ok\ntest build ... ok\ntest lint ... ok",
                ),
                |p| {
                    p.input_text = Some("make deploy");
                },
            )?,
            "terminal-input",
        )?;

        let segments = header::parse_format("${status:-10}");
        let agents = vec![make_agent_with_meta(
            "s1",
            "/home/user/myapp",
            "working",
            serde_json::json!({
                "terminal": {"type": "tmux", "server": "default", "pane_id": "%0"}
            }),
        )];
        save_screenshot(
            &make_terminal(
                80,
                18,
                &agents,
                &segments,
                Some("$ make build\nCompiling arbor-tui v0.1.0\n    Finished dev profile"),
                |p| {
                    p.table_collapsed = true;
                    p.meta_collapsed = true;
                },
            )?,
            "collapsed-panels",
        )?;

        let segments = header::parse_format("${project:-16} ${status:-8}");
        let agents = vec![make_agent_with_meta(
            "s1",
            "/home/user/myapp",
            "working",
            serde_json::json!({"project": "myapp"}),
        )];
        save_screenshot(
            &make_terminal(80, 22, &agents, &segments, None, |p| {
                p.show_help = true;
            })?,
            "help-overlay",
        )?;
        Ok(())
    }

    #[test]
    fn snapshot_rich_metadata_columns() -> Result<(), Box<dyn std::error::Error>> {
        let segments = header::parse_format(
            "${pid:6} ${session_id:8} ${cwd:-20} ${project:-12} ${branch:-12} ${status:-8} ${elapsed:>6}",
        );
        let agents = vec![
            make_agent_with_meta(
                "abc12345",
                "/home/user/frontend",
                "working",
                serde_json::json!({"pid": "9001", "project": "frontend", "branch": "main"}),
            ),
            make_agent_with_meta(
                "def67890",
                "/home/user/backend",
                "idle",
                serde_json::json!({"pid": "9002", "project": "backend", "branch": "feat/api"}),
            ),
            make_agent_with_meta(
                "ghi11111",
                "/home/user/infra",
                "working",
                serde_json::json!({"pid": "9003", "project": "infra", "branch": "fix/deploy", "blocked_on": "review"}),
            ),
        ];
        let output = draw_agents(120, 25, &agents, &segments, None)?;
        insta::assert_snapshot!(output);
        Ok(())
    }
}
