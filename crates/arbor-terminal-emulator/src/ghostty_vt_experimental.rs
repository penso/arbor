#![allow(unsafe_code)]

use {
    crate::{
        TERMINAL_COLS, TERMINAL_ROWS, TERMINAL_SCROLLBACK, TerminalCursor, TerminalModes,
        TerminalSnapshot, TerminalStyledLine, alacritty_support,
    },
    std::{ffi::c_void, ptr},
};

#[repr(C)]
struct GhosttyBuffer {
    ptr: *mut u8,
    len: usize,
}

#[link(name = "arbor_ghostty_vt_bridge")]
unsafe extern "C" {
    fn arbor_ghostty_vt_new(rows: u16, cols: u16, scrollback: usize, out: *mut *mut c_void) -> i32;
    fn arbor_ghostty_vt_free(handle: *mut c_void);
    fn arbor_ghostty_vt_process(handle: *mut c_void, bytes: *const u8, len: usize) -> i32;
    fn arbor_ghostty_vt_resize(handle: *mut c_void, rows: u16, cols: u16) -> i32;
    fn arbor_ghostty_vt_snapshot_plain(handle: *mut c_void, out: *mut GhosttyBuffer) -> i32;
    fn arbor_ghostty_vt_snapshot_vt(handle: *mut c_void, out: *mut GhosttyBuffer) -> i32;
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
}

pub struct TerminalEmulator {
    handle: *mut c_void,
    rows: u16,
    cols: u16,
}

impl TerminalEmulator {
    pub fn new() -> Self {
        Self::with_size(TERMINAL_ROWS, TERMINAL_COLS)
    }

    pub fn with_size(rows: u16, cols: u16) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(2);
        let mut handle = ptr::null_mut();
        let status = unsafe { arbor_ghostty_vt_new(rows, cols, TERMINAL_SCROLLBACK, &mut handle) };
        assert_eq!(
            status, 0,
            "failed to initialize ghostty-vt experimental terminal bridge",
        );
        assert!(
            !handle.is_null(),
            "ghostty-vt bridge returned a null terminal handle",
        );

        Self { handle, rows, cols }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let _ = unsafe { arbor_ghostty_vt_process(self.handle, bytes.as_ptr(), bytes.len()) };
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(2);
        let status = unsafe { arbor_ghostty_vt_resize(self.handle, rows, cols) };
        if status == 0 {
            self.rows = rows;
            self.cols = cols;
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
        let vt_snapshot = self
            .read_string(arbor_ghostty_vt_snapshot_vt)
            .unwrap_or_default();
        let replay = alacritty_support::replay_ansi(self.rows, self.cols, vt_snapshot.as_bytes());
        alacritty_support::collect_styled_lines(&replay)
    }

    pub fn render_ansi_snapshot(&self, max_lines: usize) -> String {
        let vt_snapshot = self
            .read_string(arbor_ghostty_vt_snapshot_vt)
            .unwrap_or_default();
        let replay = alacritty_support::replay_ansi(self.rows, self.cols, vt_snapshot.as_bytes());
        alacritty_support::render_ansi_snapshot(&replay, max_lines)
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        TerminalSnapshot {
            output: self.snapshot_output(),
            styled_lines: self.collect_styled_lines(),
            cursor: self.snapshot_cursor(),
            modes: self.snapshot_modes(),
            exit_code: None,
        }
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
            emulator.process(line.as_bytes());
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
            emulator.process(line.as_bytes());
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
        emulator.process("A\u{2600}\u{fe0f}B\r\n".as_bytes());

        let styled_lines = emulator.collect_styled_lines();
        let rendered = styled_line_to_string(styled_lines.first());

        assert_eq!(rendered, "A\u{2600}\u{fe0f}B");
    }

    #[test]
    fn snapshot_cursor_respects_cursor_visibility_mode() {
        let mut emulator = TerminalEmulator::new();
        assert!(emulator.snapshot_cursor().is_some());

        emulator.process("\u{1b}[?25l".as_bytes());
        assert!(emulator.snapshot_cursor().is_none());

        emulator.process("\u{1b}[?25h".as_bytes());
        assert!(emulator.snapshot_cursor().is_some());
    }

    #[test]
    fn snapshot_modes_track_terminal_state() {
        let mut emulator = TerminalEmulator::new();
        assert_eq!(emulator.snapshot_modes(), TerminalModes::default());

        emulator.process("\u{1b}[?1h".as_bytes());
        assert_eq!(emulator.snapshot_modes(), TerminalModes {
            app_cursor: true,
            alt_screen: false,
        });

        emulator.process("\u{1b}[?1049h".as_bytes());
        assert_eq!(emulator.snapshot_modes(), TerminalModes {
            app_cursor: true,
            alt_screen: true,
        });

        emulator.process("\u{1b}[?1l\u{1b}[?1049l".as_bytes());
        assert_eq!(emulator.snapshot_modes(), TerminalModes::default());
    }

    #[test]
    fn osc_1337_bel_terminated_silently_consumed() {
        let mut emulator = TerminalEmulator::new();
        let seq =
            "\x1b]1337;RemoteHost=penso@m4max\x07\x1b]1337;CurrentDir=/home\x07\x1b]133;C\x07";
        emulator.process(seq.as_bytes());
        let rendered = styled_lines_to_string(&emulator.collect_styled_lines());
        assert!(
            !rendered.contains("1337"),
            "BEL-terminated OSC leaked: {rendered:?}",
        );
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
