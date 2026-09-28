use {arbor_daemon_client::AgentSessionDto, ratatui::prelude::*, std::collections::HashMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Literal(String),
    Field { name: String, width: Option<i32> },
}

pub fn parse_format(fmt: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut rest = fmt;

    while let Some(start) = rest.find("${") {
        if start > 0 {
            segments.push(Segment::Literal(rest[..start].to_owned()));
        }
        let after = &rest[start + 2..];
        if let Some(end) = after.find('}') {
            let inner = &after[..end];
            let (name, width) = if let Some(colon) = inner.find(':') {
                let width_str = &inner[colon + 1..];
                let w = if let Some(rest) = width_str.strip_prefix('>') {
                    rest.parse::<i32>().ok()
                } else {
                    width_str.parse::<i32>().ok()
                };
                (inner[..colon].to_owned(), w)
            } else {
                (inner.to_owned(), None)
            };
            segments.push(Segment::Field { name, width });
            rest = &after[end + 1..];
        } else {
            segments.push(Segment::Literal(rest[start..].to_owned()));
            rest = "";
        }
    }

    if !rest.is_empty() {
        segments.push(Segment::Literal(rest.to_owned()));
    }

    segments
}

pub fn resolve_field(agent: &AgentSessionDto, name: &str) -> Option<String> {
    match name {
        "session_id" => Some(agent.session_id.clone()),
        "cwd" => Some(agent.cwd.clone()),
        "status" => Some(agent.state.clone()),
        "elapsed" => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let elapsed_secs = now_ms.saturating_sub(agent.updated_at_unix_ms) / 1000;
            Some(format_elapsed(elapsed_secs))
        },
        _ => {
            let meta = agent.metadata.as_ref()?;
            let val = meta.get(name)?;
            match val {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                serde_json::Value::Bool(b) => Some(b.to_string()),
                serde_json::Value::Null => None,
                _ => Some(val.to_string()),
            }
        },
    }
}

fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[derive(Debug, Clone)]
pub enum FieldColor {
    Static(Color),
    Map(HashMap<String, Color>),
}

pub fn resolve_color(
    field_colors: &HashMap<String, FieldColor>,
    field_name: &str,
    value: &str,
) -> Option<Color> {
    match field_colors.get(field_name)? {
        FieldColor::Static(c) => Some(*c),
        FieldColor::Map(m) => m.get(value).copied(),
    }
}

pub fn parse_color(name: &str) -> Option<Color> {
    match name {
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "cyan" => Some(Color::Cyan),
        "magenta" => Some(Color::Magenta),
        "gray" => Some(Color::Gray),
        "dark_gray" => Some(Color::DarkGray),
        "white" => Some(Color::White),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_agent(state: &str, updated_ms: u64) -> AgentSessionDto {
        AgentSessionDto {
            session_id: "sess-1".to_owned(),
            cwd: "/home/user/project".to_owned(),
            state: state.to_owned(),
            updated_at_unix_ms: updated_ms,
            metadata: Some(serde_json::json!({
                "project": "arbor",
                "branch": "feat/tmux",
                "blocked_on": "tool_use",
                "terminal": { "type": "tmux", "server": "default", "pane_id": "%0" }
            })),
        }
    }

    #[test]
    fn parse_format_simple() {
        let segs = parse_format("${status:-10} ${elapsed:>8}");
        assert_eq!(segs, vec![
            Segment::Field {
                name: "status".to_owned(),
                width: Some(-10)
            },
            Segment::Literal(" ".to_owned()),
            Segment::Field {
                name: "elapsed".to_owned(),
                width: Some(8)
            },
        ]);
    }

    #[test]
    fn parse_format_no_width() {
        let segs = parse_format("${project} | ${branch}");
        assert_eq!(segs, vec![
            Segment::Field {
                name: "project".to_owned(),
                width: None
            },
            Segment::Literal(" | ".to_owned()),
            Segment::Field {
                name: "branch".to_owned(),
                width: None
            },
        ]);
    }

    #[test]
    fn parse_format_literal_only() {
        let segs = parse_format("hello world");
        assert_eq!(segs, vec![Segment::Literal("hello world".to_owned())]);
    }

    #[test]
    fn resolve_builtin_fields() {
        let agent = make_agent("working", 0);
        assert_eq!(
            resolve_field(&agent, "session_id"),
            Some("sess-1".to_owned())
        );
        assert_eq!(
            resolve_field(&agent, "cwd"),
            Some("/home/user/project".to_owned())
        );
        assert_eq!(resolve_field(&agent, "status"), Some("working".to_owned()));
    }

    #[test]
    fn resolve_metadata_fields() {
        let agent = make_agent("working", 0);
        assert_eq!(resolve_field(&agent, "project"), Some("arbor".to_owned()));
        assert_eq!(
            resolve_field(&agent, "branch"),
            Some("feat/tmux".to_owned())
        );
        assert_eq!(
            resolve_field(&agent, "blocked_on"),
            Some("tool_use".to_owned())
        );
    }

    #[test]
    fn resolve_missing_field() {
        let agent = make_agent("working", 0);
        assert_eq!(resolve_field(&agent, "nonexistent"), None);
    }

    #[test]
    fn resolve_no_metadata() {
        let agent = AgentSessionDto {
            session_id: "s1".to_owned(),
            cwd: "/tmp".to_owned(),
            state: "idle".to_owned(),
            updated_at_unix_ms: 0,
            metadata: None,
        };
        assert_eq!(resolve_field(&agent, "project"), None);
        assert_eq!(resolve_field(&agent, "status"), Some("idle".to_owned()));
    }

    #[test]
    fn resolve_elapsed() -> Result<(), Box<dyn std::error::Error>> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as u64;
        let agent = make_agent("working", now_ms - 125_000);
        let elapsed = resolve_field(&agent, "elapsed").ok_or("expected Some elapsed")?;
        assert_eq!(elapsed, "2m");
        Ok(())
    }

    #[test]
    fn format_elapsed_values() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(45), "45s");
        assert_eq!(format_elapsed(120), "2m");
        assert_eq!(format_elapsed(3661), "1h1m");
    }

    #[test]
    fn color_static() {
        let mut colors = HashMap::new();
        colors.insert("project".to_owned(), FieldColor::Static(Color::Cyan));
        assert_eq!(
            resolve_color(&colors, "project", "anything"),
            Some(Color::Cyan)
        );
    }

    #[test]
    fn color_map_match() {
        let mut colors = HashMap::new();
        colors.insert(
            "status".to_owned(),
            FieldColor::Map([("working".to_owned(), Color::Green)].into_iter().collect()),
        );
        assert_eq!(
            resolve_color(&colors, "status", "working"),
            Some(Color::Green)
        );
        assert_eq!(resolve_color(&colors, "status", "unknown"), None);
    }

    #[test]
    fn color_missing_field() {
        let colors = HashMap::new();
        assert_eq!(resolve_color(&colors, "nope", "val"), None);
    }
}
