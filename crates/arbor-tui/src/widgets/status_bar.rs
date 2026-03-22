use ratatui::{prelude::*, widgets::*};

pub fn render_status_bar(
    frame: &mut Frame,
    area: Rect,
    connected: bool,
    last_poll_secs: Option<u64>,
    action_hints: &[(char, &str)],
) {
    let mut spans = Vec::new();

    if connected {
        spans.push(Span::styled(
            "● Connected",
            Style::default().fg(Color::Green),
        ));
    } else {
        spans.push(Span::styled(
            "● Disconnected",
            Style::default().fg(Color::Red),
        ));
    }

    if let Some(secs) = last_poll_secs {
        spans.push(Span::raw(format!("  Poll: {secs}s ago")));
    }

    spans.push(Span::raw("  "));
    spans.push(Span::styled("q", Style::default().fg(Color::Yellow)));
    spans.push(Span::raw(":quit "));
    spans.push(Span::styled("j/k", Style::default().fg(Color::Yellow)));
    spans.push(Span::raw(":nav"));

    for (key, name) in action_hints {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("{key}"),
            Style::default().fg(Color::Cyan),
        ));
        spans.push(Span::raw(format!(":{name}")));
    }

    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        ratatui::{Terminal, backend::TestBackend},
    };

    fn render_bar(
        connected: bool,
        poll_secs: Option<u64>,
        action_hints: &[(char, &str)],
    ) -> Result<String, Box<dyn std::error::Error>> {
        let backend = TestBackend::new(100, 1);
        let mut terminal = Terminal::new(backend)?;

        terminal.draw(|frame| {
            render_status_bar(frame, frame.area(), connected, poll_secs, action_hints);
        })?;

        let buf = terminal.backend().buffer().clone();
        Ok((0..100).map(|x| buf[(x, 0)].symbol().to_string()).collect())
    }

    #[test]
    fn test_status_bar_shows_connected() -> Result<(), Box<dyn std::error::Error>> {
        let text = render_bar(true, Some(3), &[])?;
        assert!(text.contains("Connected"), "connected status missing");
        assert!(text.contains("3s ago"), "poll time missing");
        Ok(())
    }

    #[test]
    fn test_status_bar_shows_disconnected() -> Result<(), Box<dyn std::error::Error>> {
        let text = render_bar(false, None, &[])?;
        assert!(text.contains("Disconnected"), "disconnected status missing");
        Ok(())
    }

    #[test]
    fn test_status_bar_shows_builtin_hints() -> Result<(), Box<dyn std::error::Error>> {
        let text = render_bar(true, None, &[])?;
        assert!(text.contains("quit"), "quit hint missing");
        assert!(text.contains("nav"), "nav hint missing");
        Ok(())
    }

    #[test]
    fn test_status_bar_shows_action_hints() -> Result<(), Box<dyn std::error::Error>> {
        let text = render_bar(true, None, &[('g', "goto"), ('o', "open")])?;
        assert!(text.contains("goto"), "action hint 'goto' missing");
        assert!(text.contains("open"), "action hint 'open' missing");
        Ok(())
    }
}
