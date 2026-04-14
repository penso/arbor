use {
    crate::{
        TerminalModes, TerminalProcessReport, TerminalSnapshot,
        alacritty_support::{
            self, collect_styled_lines, collect_styled_lines_tail, new_state, snapshot_cursor,
            snapshot_cursor_tail, snapshot_modes, snapshot_output, snapshot_output_tail,
        },
    },
    std::cell::RefCell,
};

#[derive(Clone)]
struct CachedOutputSnapshot {
    generation: u64,
    output: String,
}

#[derive(Clone)]
struct CachedStyledSnapshot {
    generation: u64,
    styled_lines: Vec<crate::TerminalStyledLine>,
    cursor: Option<crate::TerminalCursor>,
    modes: TerminalModes,
}

pub struct TerminalEmulator {
    state: alacritty_support::AlacrittyState,
    generation: u64,
    output_snapshot_cache: RefCell<Option<CachedOutputSnapshot>>,
    styled_snapshot_cache: RefCell<Option<CachedStyledSnapshot>>,
}

impl TerminalEmulator {
    pub fn new() -> Self {
        Self::with_size(crate::TERMINAL_ROWS, crate::TERMINAL_COLS)
    }

    pub fn with_size(rows: u16, cols: u16) -> Self {
        Self {
            state: new_state(rows, cols),
            generation: 0,
            output_snapshot_cache: RefCell::new(None),
            styled_snapshot_cache: RefCell::new(None),
        }
    }

    pub fn process_and_report(&mut self, bytes: &[u8]) -> TerminalProcessReport {
        if bytes.is_empty() {
            return TerminalProcessReport::default();
        }

        self.state.processor.advance(&mut self.state.term, bytes);
        let report = self.state.event_listener.take_process_report();
        self.generation = self.generation.saturating_add(1);
        self.output_snapshot_cache.get_mut().take();
        self.styled_snapshot_cache.get_mut().take();
        report
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let dimensions = alacritty_support::TerminalDimensions {
            rows: usize::from(rows),
            cols: usize::from(cols),
        };
        self.state.term.resize(dimensions);
        self.generation = self.generation.saturating_add(1);
        self.output_snapshot_cache.get_mut().take();
        self.styled_snapshot_cache.get_mut().take();
    }

    pub fn snapshot_output(&self) -> String {
        self.output_snapshot().output
    }

    pub fn snapshot_cursor(&self) -> Option<crate::TerminalCursor> {
        self.styled_snapshot().cursor
    }

    pub fn snapshot_modes(&self) -> TerminalModes {
        self.styled_snapshot().modes
    }

    pub fn collect_styled_lines(&self) -> Vec<crate::TerminalStyledLine> {
        self.styled_snapshot().styled_lines
    }

    pub fn render_ansi_snapshot(&self, max_lines: usize) -> String {
        let snapshot = self.styled_snapshot();
        alacritty_support::render_ansi_from_styled_lines(
            &snapshot.styled_lines,
            snapshot.cursor,
            max_lines,
        )
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let output_snapshot = self.output_snapshot();
        let styled_snapshot = self.styled_snapshot();
        TerminalSnapshot {
            output: output_snapshot.output,
            styled_lines: styled_snapshot.styled_lines,
            cursor: styled_snapshot.cursor,
            modes: styled_snapshot.modes,
            exit_code: None,
        }
    }

    pub fn snapshot_tail(&self, max_lines: usize) -> TerminalSnapshot {
        TerminalSnapshot {
            output: snapshot_output_tail(&self.state.term, max_lines),
            styled_lines: collect_styled_lines_tail(&self.state.term, max_lines),
            cursor: snapshot_cursor_tail(&self.state.term, max_lines),
            modes: snapshot_modes(&self.state.term),
            exit_code: None,
        }
    }

    fn output_snapshot(&self) -> CachedOutputSnapshot {
        if let Some(snapshot) = self.cached_output_snapshot() {
            return snapshot;
        }

        let snapshot = CachedOutputSnapshot {
            generation: self.generation,
            output: snapshot_output(&self.state.term),
        };
        *self.output_snapshot_cache.borrow_mut() = Some(snapshot.clone());
        snapshot
    }

    fn styled_snapshot(&self) -> CachedStyledSnapshot {
        if let Some(snapshot) = self.cached_styled_snapshot() {
            return snapshot;
        }

        let snapshot = CachedStyledSnapshot {
            generation: self.generation,
            styled_lines: collect_styled_lines(&self.state.term),
            cursor: snapshot_cursor(&self.state.term),
            modes: snapshot_modes(&self.state.term),
        };
        *self.styled_snapshot_cache.borrow_mut() = Some(snapshot.clone());
        snapshot
    }

    fn cached_output_snapshot(&self) -> Option<CachedOutputSnapshot> {
        self.output_snapshot_cache
            .borrow()
            .as_ref()
            .filter(|snapshot| snapshot.generation == self.generation)
            .cloned()
    }

    fn cached_styled_snapshot(&self) -> Option<CachedStyledSnapshot> {
        self.styled_snapshot_cache
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
        let last = styled_lines
            .iter()
            .rev()
            .map(styled_line_to_string_ref)
            .find(|s| !s.is_empty())
            .unwrap_or_default();

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
    fn cursor_relative_redraw_replaces_prompt_lines_cleanly() {
        let mut emulator = TerminalEmulator::with_size(12, 72);
        let initial = concat!(
            "  Would you like to make the following edits?\r\n",
            "\r\n",
            "  crates/arbor-gui/src/app_init.rs (+4 -0)\r\n",
            "    223  -        self.terminal_scroll_handle: ScrollHandle::new(),\r\n",
            "    224  +        terminal_follow_output_until: None,\r\n",
            "    225  +        last_terminal_scroll_offset_y: None,\r\n",
            "\r\n",
            "  1. Yes, proceed (y)\r\n",
            "› 2. Yes, and don't ask again for these files (a)\r\n",
            "  3. No, and tell Codex what to do differently (esc)",
        );
        let redraw = concat!(
            "\x1b[3A",
            "\r\x1b[2K  1. Yes, proceed (y)",
            "\x1b[1B",
            "\r\x1b[2K› 2. Yes, and don't ask again for these files (a)",
            "\x1b[1B",
            "\r\x1b[2K  3. No, and tell Codex what to do differently (esc)",
        );

        let _ = emulator.process_and_report(initial.as_bytes());
        let _ = emulator.process_and_report(redraw.as_bytes());

        let snapshot = emulator.snapshot_output();
        assert!(
            snapshot.contains("Would you like to make the following edits?"),
            "expected prompt header to survive redraws: {snapshot:?}"
        );
        assert!(
            snapshot.contains("terminal_follow_output_until: None"),
            "expected file diff content to remain intact: {snapshot:?}"
        );
        assert!(
            snapshot.contains("› 2. Yes, and don't ask again for these files (a)"),
            "expected selected option to render cleanly after redraw: {snapshot:?}"
        );
        assert!(
            snapshot.contains("  3. No, and tell Codex what to do differently (esc)"),
            "expected final menu option to remain readable: {snapshot:?}"
        );
        assert!(
            !snapshot.contains("1. Yes, and don't ask again"),
            "unexpected prompt line bleed after redraw: {snapshot:?}"
        );
    }

    #[test]
    fn clear_screen_snapshot_preserves_blank_rows_for_visible_screen() {
        let mut emulator = TerminalEmulator::with_size(12, 72);
        let frame = concat!(
            "\x1b[H\x1b[2J",
            "  Would you like to make the following edits?\r\n",
            "\r\n",
            "  crates/arbor-gui/src/app_init.rs (+4 -0)\r\n",
            "    223  -        self.terminal_scroll_handle: ScrollHandle::new(),\r\n",
            "    224  +        terminal_follow_output_until: None,\r\n",
            "    225  +        last_terminal_scroll_offset_y: None,\r\n",
            "\r\n",
            "  1. Yes, proceed (y)\r\n",
            "› 2. Yes, and don't ask again for these files (a)\r\n",
            "  3. No, and tell Codex what to do differently (esc)",
        );

        let _ = emulator.process_and_report(frame.as_bytes());

        let snapshot = emulator.snapshot();
        assert!(
            snapshot.styled_lines.len() >= 12,
            "expected visible screen rows to remain in snapshot: {:?}",
            snapshot.styled_lines
        );
        assert!(
            snapshot
                .styled_lines
                .iter()
                .take(2)
                .any(|line| styled_line_to_string(Some(line))
                    .contains("Would you like to make the following edits?")),
            "expected prompt header to remain at the top of the visible frame: {:?}",
            snapshot.styled_lines
        );
        assert!(
            snapshot
                .styled_lines
                .last()
                .is_some_and(|line| line.cells.is_empty()),
            "expected trailing blank screen rows to remain in snapshot: {:?}",
            snapshot.styled_lines
        );
        assert_eq!(
            snapshot.cursor.map(|cursor| cursor.line),
            Some(10),
            "expected cursor to stay on the active prompt option: {:?}",
            snapshot.cursor
        );
    }

    #[test]
    fn wide_scroll_snapshot_preserves_full_rows_without_missing_chars() {
        let mut emulator = TerminalEmulator::with_size(48, 120);
        let _ = emulator.process_and_report(
            b"\x1b[H\x1b[2JFilesystem             Size   Used  Avail Capacity Mounted on\r\n",
        );

        for row in 0..220 {
            let used_gib = (row * 7) % 900 + 50;
            let avail_gib = 1024 - used_gib;
            let capacity = (used_gib * 100) / 1024;
            let line = format!(
                "/dev/disk{row:<3}         1.0Ti  {used_gib:>4}Gi  {avail_gib:>4}Gi    {capacity:>2}%   /Volumes/worktree-{row:03}\r\n"
            );
            let _ = emulator.process_and_report(line.as_bytes());
        }

        let snapshot = emulator.snapshot();
        let lines = snapshot
            .styled_lines
            .iter()
            .map(|line| styled_line_to_string(Some(line)))
            .collect::<Vec<_>>();
        let expected_last_row =
            "/dev/disk219         1.0Ti   683Gi   341Gi    66%   /Volumes/worktree-219";

        assert!(
            lines
                .iter()
                .any(|line| line == "Filesystem             Size   Used  Avail Capacity Mounted on"),
            "expected df-like header to survive snapshot: {lines:?}"
        );
        assert_eq!(
            lines.iter().rev().find(|line| !line.is_empty()),
            Some(&expected_last_row.to_owned()),
            "expected final df-like row to survive snapshot without missing chars: {lines:?}"
        );
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

    fn styled_line_to_string_ref(line: &crate::TerminalStyledLine) -> String {
        line.runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>()
    }

    fn styled_lines_to_string(lines: &[crate::TerminalStyledLine]) -> String {
        lines
            .iter()
            .flat_map(|line| line.runs.iter())
            .map(|run| run.text.as_str())
            .collect()
    }
}
