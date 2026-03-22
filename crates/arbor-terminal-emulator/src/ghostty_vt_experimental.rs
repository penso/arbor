#![allow(unsafe_code)]

use {
    crate::{
        TERMINAL_COLS, TERMINAL_ROWS, TerminalCursor, TerminalModes, TerminalProcessReport,
        TerminalSnapshot, TerminalStyledCell, TerminalStyledLine, alacritty_support,
        default_terminal_scrollback_lines,
    },
    std::{cell::RefCell, ffi::c_void, ptr},
};

#[repr(C)]
struct GhosttyBuffer {
    ptr: *mut u8,
    len: usize,
}

#[repr(C)]
struct GhosttyStyledLine {
    cell_start: usize,
    cell_len: usize,
}

#[repr(C)]
struct GhosttyStyledCell {
    column: usize,
    text_offset: usize,
    text_len: usize,
    fg: u32,
    bg: u32,
}

#[repr(C)]
struct GhosttyStyledSnapshot {
    lines_ptr: *mut GhosttyStyledLine,
    lines_len: usize,
    cells_ptr: *mut GhosttyStyledCell,
    cells_len: usize,
    text_ptr: *mut u8,
    text_len: usize,
    cursor_visible: bool,
    cursor_line: usize,
    cursor_column: usize,
    app_cursor: bool,
    alt_screen: bool,
}

#[derive(Clone)]
struct CachedStyledSnapshot {
    generation: u64,
    styled_lines: Vec<TerminalStyledLine>,
    cursor: Option<TerminalCursor>,
    modes: TerminalModes,
}

#[link(name = "arbor_ghostty_vt_bridge")]
unsafe extern "C" {
    fn arbor_ghostty_vt_new(rows: u16, cols: u16, scrollback: usize, out: *mut *mut c_void) -> i32;
    fn arbor_ghostty_vt_free(handle: *mut c_void);
    fn arbor_ghostty_vt_process(
        handle: *mut c_void,
        bytes: *const u8,
        len: usize,
        bell_count: *mut usize,
    ) -> i32;
    fn arbor_ghostty_vt_resize(handle: *mut c_void, rows: u16, cols: u16) -> i32;
    fn arbor_ghostty_vt_snapshot_plain(handle: *mut c_void, out: *mut GhosttyBuffer) -> i32;
    fn arbor_ghostty_vt_snapshot_styled(
        handle: *mut c_void,
        out: *mut GhosttyStyledSnapshot,
    ) -> i32;
    fn arbor_ghostty_vt_snapshot_cursor(
        handle: *mut c_void,
        visible: *mut bool,
        line: *mut usize,
        column: *mut usize,
    ) -> i32;
    fn arbor_ghostty_vt_snapshot_modes(
        handle: *mut c_void,
        app_cursor: *mut bool,
        alt_screen: *mut bool,
    ) -> i32;
    fn arbor_ghostty_vt_free_buffer(buffer: GhosttyBuffer);
    fn arbor_ghostty_vt_free_styled_snapshot(snapshot: GhosttyStyledSnapshot);
}

pub struct TerminalEmulator {
    handle: *mut c_void,
    generation: u64,
    styled_snapshot_cache: RefCell<Option<CachedStyledSnapshot>>,
}

impl TerminalEmulator {
    pub fn new() -> Self {
        Self::with_size(TERMINAL_ROWS, TERMINAL_COLS)
    }

    pub fn with_size(rows: u16, cols: u16) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(2);
        let mut handle = ptr::null_mut();
        let status = unsafe {
            arbor_ghostty_vt_new(rows, cols, default_terminal_scrollback_lines(), &mut handle)
        };
        assert_eq!(
            status, 0,
            "failed to initialize ghostty-vt experimental terminal bridge",
        );
        assert!(
            !handle.is_null(),
            "ghostty-vt bridge returned a null terminal handle",
        );

        Self {
            handle,
            generation: 0,
            styled_snapshot_cache: RefCell::new(None),
        }
    }

    pub fn process_and_report(&mut self, bytes: &[u8]) -> TerminalProcessReport {
        if bytes.is_empty() {
            return TerminalProcessReport::default();
        }
        let mut bell_count = 0;
        let _ = unsafe {
            arbor_ghostty_vt_process(self.handle, bytes.as_ptr(), bytes.len(), &mut bell_count)
        };
        self.generation = self.generation.saturating_add(1);
        self.styled_snapshot_cache.get_mut().take();
        TerminalProcessReport { bell_count }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(2);
        let status = unsafe { arbor_ghostty_vt_resize(self.handle, rows, cols) };
        if status == 0 {
            self.generation = self.generation.saturating_add(1);
            self.styled_snapshot_cache.get_mut().take();
        }
    }

    pub fn snapshot_output(&self) -> String {
        self.read_string(arbor_ghostty_vt_snapshot_plain)
            .unwrap_or_default()
    }

    pub fn snapshot_cursor(&self) -> Option<TerminalCursor> {
        let mut visible = false;
        let mut line = 0;
        let mut column = 0;
        let status = unsafe {
            arbor_ghostty_vt_snapshot_cursor(self.handle, &mut visible, &mut line, &mut column)
        };
        if status != 0 || !visible {
            return None;
        }

        Some(TerminalCursor { line, column })
    }

    pub fn snapshot_modes(&self) -> TerminalModes {
        if let Some(snapshot) = self.cached_styled_snapshot() {
            return snapshot.modes;
        }

        let mut app_cursor = false;
        let mut alt_screen = false;
        let status = unsafe {
            arbor_ghostty_vt_snapshot_modes(self.handle, &mut app_cursor, &mut alt_screen)
        };
        if status != 0 {
            return TerminalModes::default();
        }

        TerminalModes {
            app_cursor,
            alt_screen,
        }
    }

    pub fn collect_styled_lines(&self) -> Vec<TerminalStyledLine> {
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
        let styled_snapshot = self.styled_snapshot();
        TerminalSnapshot {
            output: self.snapshot_output(),
            styled_lines: styled_snapshot.styled_lines,
            cursor: styled_snapshot.cursor,
            modes: styled_snapshot.modes,
            exit_code: None,
        }
    }

    pub fn snapshot_tail(&self, max_lines: usize) -> TerminalSnapshot {
        let mut snapshot = self.snapshot();
        if max_lines == 0 {
            snapshot.output.clear();
            snapshot.styled_lines.clear();
            snapshot.cursor = None;
            return snapshot;
        }

        snapshot.output = snapshot
            .output
            .lines()
            .rev()
            .take(max_lines)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");

        let keep_from = snapshot.styled_lines.len().saturating_sub(max_lines);
        snapshot.cursor = snapshot.cursor.and_then(|cursor| {
            (cursor.line >= keep_from).then_some(TerminalCursor {
                line: cursor.line - keep_from,
                column: cursor.column,
            })
        });
        if keep_from > 0 {
            snapshot.styled_lines.drain(..keep_from);
        }
        snapshot
    }

    fn styled_snapshot(&self) -> CachedStyledSnapshot {
        if let Some(snapshot) = self.cached_styled_snapshot() {
            return snapshot;
        }

        let mut raw_snapshot = GhosttyStyledSnapshot {
            lines_ptr: ptr::null_mut(),
            lines_len: 0,
            cells_ptr: ptr::null_mut(),
            cells_len: 0,
            text_ptr: ptr::null_mut(),
            text_len: 0,
            cursor_visible: false,
            cursor_line: 0,
            cursor_column: 0,
            app_cursor: false,
            alt_screen: false,
        };

        let status = unsafe { arbor_ghostty_vt_snapshot_styled(self.handle, &mut raw_snapshot) };
        if status != 0 {
            return CachedStyledSnapshot {
                generation: self.generation,
                styled_lines: vec![alacritty_support::finalize_styled_line(Vec::new())],
                cursor: None,
                modes: TerminalModes::default(),
            };
        }

        let snapshot = self.decode_styled_snapshot(raw_snapshot);
        *self.styled_snapshot_cache.borrow_mut() = Some(snapshot.clone());
        snapshot
    }

    fn cached_styled_snapshot(&self) -> Option<CachedStyledSnapshot> {
        self.styled_snapshot_cache
            .borrow()
            .as_ref()
            .filter(|snapshot| snapshot.generation == self.generation)
            .cloned()
    }

    fn read_string(
        &self,
        fetch: unsafe extern "C" fn(*mut c_void, *mut GhosttyBuffer) -> i32,
    ) -> Option<String> {
        let mut buffer = GhosttyBuffer {
            ptr: ptr::null_mut(),
            len: 0,
        };
        let status = unsafe { fetch(self.handle, &mut buffer) };
        if status != 0 {
            return None;
        }

        let bytes = unsafe { std::slice::from_raw_parts(buffer.ptr, buffer.len) };
        let text = String::from_utf8_lossy(bytes).into_owned();
        unsafe {
            arbor_ghostty_vt_free_buffer(buffer);
        }
        Some(text)
    }

    fn decode_styled_snapshot(&self, snapshot: GhosttyStyledSnapshot) -> CachedStyledSnapshot {
        let raw_lines = raw_slice(snapshot.lines_ptr, snapshot.lines_len);
        let raw_cells = raw_slice(snapshot.cells_ptr, snapshot.cells_len);
        let raw_text = raw_slice(snapshot.text_ptr, snapshot.text_len);

        let mut styled_lines = Vec::with_capacity(raw_lines.len());
        for raw_line in raw_lines {
            let start = raw_line.cell_start.min(raw_cells.len());
            let end = start.saturating_add(raw_line.cell_len).min(raw_cells.len());
            let mut cells = Vec::with_capacity(end.saturating_sub(start));

            for raw_cell in &raw_cells[start..end] {
                let text_start = raw_cell.text_offset.min(raw_text.len());
                let text_end = text_start
                    .saturating_add(raw_cell.text_len)
                    .min(raw_text.len());
                let text = String::from_utf8_lossy(&raw_text[text_start..text_end]).into_owned();
                cells.push(TerminalStyledCell {
                    column: raw_cell.column,
                    text,
                    fg: raw_cell.fg,
                    bg: raw_cell.bg,
                });
            }

            styled_lines.push(alacritty_support::finalize_styled_line(cells));
        }

        while styled_lines
            .last()
            .is_some_and(|line| line.cells.is_empty())
        {
            styled_lines.pop();
        }

        if styled_lines.is_empty() {
            styled_lines.push(alacritty_support::finalize_styled_line(Vec::new()));
        }

        let cached = CachedStyledSnapshot {
            generation: self.generation,
            styled_lines,
            cursor: snapshot.cursor_visible.then_some(TerminalCursor {
                line: snapshot.cursor_line,
                column: snapshot.cursor_column,
            }),
            modes: TerminalModes {
                app_cursor: snapshot.app_cursor,
                alt_screen: snapshot.alt_screen,
            },
        };

        unsafe {
            arbor_ghostty_vt_free_styled_snapshot(snapshot);
        }

        cached
    }
}

fn raw_slice<T>(ptr: *const T, len: usize) -> &'static [T] {
    if len == 0 || ptr.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

impl Default for TerminalEmulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TerminalEmulator {
    fn drop(&mut self) {
        unsafe {
            arbor_ghostty_vt_free(self.handle);
        }
    }
}

unsafe impl Send for TerminalEmulator {}

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
            "expected oldest visible scrollback in snapshot output",
        );
        assert!(
            snapshot.contains("output-219"),
            "expected latest output in snapshot output",
        );

        let snapshot_line_count = snapshot.lines().count();
        assert!(
            snapshot_line_count > usize::from(TERMINAL_ROWS),
            "expected snapshot line count ({snapshot_line_count}) to exceed viewport rows ({})",
            TERMINAL_ROWS,
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
    fn snapshot_cursor_uses_screen_coordinates_with_scrollback() {
        let mut emulator = TerminalEmulator::new();

        for line_index in 0..120 {
            let line = format!("line-{line_index:03}\r\n");
            let _ = emulator.process_and_report(line.as_bytes());
        }

        let _ = emulator.process_and_report("prompt> ".as_bytes());

        let Some(cursor) = emulator.snapshot_cursor() else {
            panic!("cursor should remain visible");
        };
        let styled_lines = emulator.collect_styled_lines();

        assert_eq!(
            cursor.line,
            styled_lines.len().saturating_sub(1),
            "cursor should point at the last rendered screen line",
        );
        assert_eq!(cursor.column, "prompt> ".chars().count());
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
            "BEL-terminated OSC leaked: {rendered:?}",
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

    fn styled_line_to_string(line: Option<&TerminalStyledLine>) -> String {
        line.map(|line| {
            line.runs
                .iter()
                .map(|run| run.text.as_str())
                .collect::<String>()
        })
        .unwrap_or_default()
    }

    fn styled_lines_to_string(lines: &[TerminalStyledLine]) -> String {
        lines
            .iter()
            .flat_map(|line| line.runs.iter())
            .map(|run| run.text.as_str())
            .collect()
    }
}
