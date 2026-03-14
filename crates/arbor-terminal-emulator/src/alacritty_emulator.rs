use {
    crate::{
        TerminalModes, TerminalProcessReport, TerminalSnapshot,
        alacritty_support::{
            self, collect_styled_lines, new_state, snapshot_cursor, snapshot_modes, snapshot_output,
        },
    },
    std::cell::RefCell,
};

#[derive(Clone)]
struct CachedSnapshot {
    generation: u64,
    output: String,
    styled_lines: Vec<crate::TerminalStyledLine>,
    cursor: Option<crate::TerminalCursor>,
    modes: TerminalModes,
}

pub struct TerminalEmulator {
    state: alacritty_support::AlacrittyState,
    generation: u64,
    snapshot_cache: RefCell<Option<CachedSnapshot>>,
}

impl TerminalEmulator {
    pub fn new() -> Self {
        Self::with_size(crate::TERMINAL_ROWS, crate::TERMINAL_COLS)
    }

    pub fn with_size(rows: u16, cols: u16) -> Self {
        Self {
            state: new_state(rows, cols),
            generation: 0,
            snapshot_cache: RefCell::new(None),
        }
    }

    pub fn process_and_report(&mut self, bytes: &[u8]) -> TerminalProcessReport {
        if bytes.is_empty() {
            return TerminalProcessReport::default();
        }

        self.state.processor.advance(&mut self.state.term, bytes);
        let report = self.state.event_listener.take_process_report();
        self.generation = self.generation.saturating_add(1);
        self.snapshot_cache.get_mut().take();
        report
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let dimensions = alacritty_support::TerminalDimensions {
            rows: usize::from(rows),
            cols: usize::from(cols),
        };
        self.state.term.resize(dimensions);
        self.generation = self.generation.saturating_add(1);
        self.snapshot_cache.get_mut().take();
    }

    pub fn snapshot_output(&self) -> String {
        self.snapshot_state().output
    }

    pub fn snapshot_cursor(&self) -> Option<crate::TerminalCursor> {
        self.snapshot_state().cursor
    }

    pub fn snapshot_modes(&self) -> TerminalModes {
        self.snapshot_state().modes
    }

    pub fn collect_styled_lines(&self) -> Vec<crate::TerminalStyledLine> {
        self.snapshot_state().styled_lines
    }

    pub fn render_ansi_snapshot(&self, max_lines: usize) -> String {
        let snapshot = self.snapshot_state();
        alacritty_support::render_ansi_from_styled_lines(
            &snapshot.styled_lines,
            snapshot.cursor,
            max_lines,
        )
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let snapshot = self.snapshot_state();
        TerminalSnapshot {
            output: snapshot.output,
            styled_lines: snapshot.styled_lines,
            cursor: snapshot.cursor,
            modes: snapshot.modes,
            exit_code: None,
        }
    }

    fn snapshot_state(&self) -> CachedSnapshot {
        if let Some(snapshot) = self.cached_snapshot() {
            return snapshot;
        }

        let snapshot = CachedSnapshot {
            generation: self.generation,
            output: snapshot_output(&self.state.term),
            styled_lines: collect_styled_lines(&self.state.term),
            cursor: snapshot_cursor(&self.state.term),
            modes: snapshot_modes(&self.state.term),
        };
        *self.snapshot_cache.borrow_mut() = Some(snapshot.clone());
        snapshot
    }

    fn cached_snapshot(&self) -> Option<CachedSnapshot> {
        self.snapshot_cache
            .borrow()
            .as_ref()
            .filter(|snapshot| snapshot.generation == self.generation)
            .cloned()
    }
}

impl Default for TerminalEmulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_lines_include_scrollback_content() {
        let mut emulator = TerminalEmulator::new();

        for line_index in 0..120 {
            let line = format!("line-{line_index:03}\r\n");
            let _ = emulator.process_and_report(line.as_bytes());
        }

        let styled_lines = emulator.collect_styled_lines();
        assert!(
            styled_lines.len() > 60,
            "expected many lines from scrollback, got {}",
            styled_lines.len()
        );

        let first = styled_line_to_string(styled_lines.first());
        let last = styled_line_to_string(styled_lines.last());

        assert!(
            first.contains("line-000"),
            "expected first scrollback line to be present, got `{first}`"
        );
        assert!(
            last.contains("line-119"),
            "expected final line to be present, got `{last}`"
        );
    }

    #[test]
    fn plain_snapshot_output_includes_scrollback_content() {
        let mut emulator = TerminalEmulator::new();

        for line_index in 0..220 {
            let line = format!("output-{line_index:03}\r\n");
            let _ = emulator.process_and_report(line.as_bytes());
        }

        let snapshot = emulator.snapshot_output();
        assert!(
            snapshot.contains("output-000"),
            "expected oldest visible scrollback in snapshot output"
        );
        assert!(
            snapshot.contains("output-219"),
            "expected latest output in snapshot output"
        );

        let snapshot_line_count = snapshot.lines().count();
        assert!(
            snapshot_line_count > usize::from(crate::TERMINAL_ROWS),
            "expected snapshot line count ({snapshot_line_count}) to exceed viewport rows ({})",
            crate::TERMINAL_ROWS,
        );
    }

    #[test]
    fn styled_lines_skip_space_after_zero_width_sequence() {
        let mut emulator = TerminalEmulator::new();
        let _ = emulator.process_and_report("A\u{2600}\u{fe0f}B\r\n".as_bytes());

        let styled_lines = emulator.collect_styled_lines();
        let rendered = styled_line_to_string(styled_lines.first());

        assert_eq!(rendered, "A\u{2600}\u{fe0f}B");
    }

    #[test]
    fn snapshot_cursor_respects_cursor_visibility_mode() {
        let mut emulator = TerminalEmulator::new();
        assert!(emulator.snapshot_cursor().is_some());

        let _ = emulator.process_and_report("\u{1b}[?25l".as_bytes());
        assert!(emulator.snapshot_cursor().is_none());

        let _ = emulator.process_and_report("\u{1b}[?25h".as_bytes());
        assert!(emulator.snapshot_cursor().is_some());
    }

    #[test]
    fn snapshot_modes_track_terminal_state() {
        let mut emulator = TerminalEmulator::new();
        assert_eq!(emulator.snapshot_modes(), TerminalModes::default());

        let _ = emulator.process_and_report("\u{1b}[?1h".as_bytes());
        assert_eq!(emulator.snapshot_modes(), TerminalModes {
            app_cursor: true,
            alt_screen: false,
        });

        let _ = emulator.process_and_report("\u{1b}[?1049h".as_bytes());
        assert_eq!(emulator.snapshot_modes(), TerminalModes {
            app_cursor: true,
            alt_screen: true,
        });

        let _ = emulator.process_and_report("\u{1b}[?1l\u{1b}[?1049l".as_bytes());
        assert_eq!(emulator.snapshot_modes(), TerminalModes::default());
    }

    #[test]
    fn osc_1337_bel_terminated_silently_consumed() {
        let mut emulator = TerminalEmulator::new();
        let seq =
            "\x1b]1337;RemoteHost=penso@m4max\x07\x1b]1337;CurrentDir=/home\x07\x1b]133;C\x07";
        let _ = emulator.process_and_report(seq.as_bytes());
        let rendered = styled_lines_to_string(&emulator.collect_styled_lines());
        assert!(
            !rendered.contains("1337"),
            "BEL-terminated OSC leaked: {rendered:?}"
        );
    }

    #[test]
    fn process_report_counts_real_bell_only() {
        let mut emulator = TerminalEmulator::new();

        let report = emulator.process_and_report("hello\x07".as_bytes());
        assert_eq!(report.bell_count, 1);
        assert!(report.bell_rang());

        let report = emulator.process_and_report(
            "\x1b]1337;RemoteHost=penso@m4max\x07\x1b]1337;CurrentDir=/home\x07".as_bytes(),
        );
        assert_eq!(report.bell_count, 0);
        assert!(!report.bell_rang());
    }

    fn styled_line_to_string(line: Option<&crate::TerminalStyledLine>) -> String {
        line.map(|line| {
            line.runs
                .iter()
                .map(|run| run.text.as_str())
                .collect::<String>()
        })
        .unwrap_or_default()
    }

    fn styled_lines_to_string(lines: &[crate::TerminalStyledLine]) -> String {
        lines
            .iter()
            .flat_map(|line| line.runs.iter())
            .map(|run| run.text.as_str())
            .collect()
    }
}
