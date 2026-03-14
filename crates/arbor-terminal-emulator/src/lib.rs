mod alacritty_emulator;
mod alacritty_support;
#[cfg(feature = "ghostty-vt-experimental")]
mod ghostty_vt_experimental;

use std::sync::atomic::{AtomicU8, Ordering};

pub const TERMINAL_ROWS: u16 = 24;
pub const TERMINAL_COLS: u16 = 80;
pub const TERMINAL_SCROLLBACK: usize = 8_000;

pub const TERMINAL_DEFAULT_FG: u32 = 0xabb2bf;
pub const TERMINAL_DEFAULT_BG: u32 = 0x282c34;
pub const TERMINAL_CURSOR: u32 = 0x74ade8;
pub const TERMINAL_BRIGHT_FG: u32 = 0xdce0e5;
pub const TERMINAL_DIM_FG: u32 = 0x636d83;
pub const TERMINAL_ANSI_16: [u32; 16] = [
    0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x636d83,
    0xea858b, 0xaad581, 0xffd885, 0x85c1ff, 0xd398eb, 0x6ed5de, 0xfafafa,
];
pub const TERMINAL_ANSI_DIM_8: [u32; 8] = [
    0x3b3f4a, 0xa7545a, 0x6d8f59, 0xb8985b, 0x457cad, 0x8d54a0, 0x3c818a, 0x8f969b,
];

#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    pub output: String,
    pub styled_lines: Vec<TerminalStyledLine>,
    pub cursor: Option<TerminalCursor>,
    pub modes: TerminalModes,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TerminalProcessReport {
    pub bell_count: usize,
}

impl TerminalProcessReport {
    pub const fn bell_rang(self) -> bool {
        self.bell_count > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursor {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TerminalModes {
    pub app_cursor: bool,
    pub alt_screen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalStyledLine {
    pub cells: Vec<TerminalStyledCell>,
    pub runs: Vec<TerminalStyledRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalStyledCell {
    pub column: usize,
    pub text: String,
    pub fg: u32,
    pub bg: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalStyledRun {
    pub text: String,
    pub fg: u32,
    pub bg: u32,
}

pub use alacritty_support::process_terminal_bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalEngineKind {
    #[cfg_attr(not(feature = "ghostty-vt-experimental"), default)]
    Alacritty,
    #[cfg(feature = "ghostty-vt-experimental")]
    #[cfg_attr(feature = "ghostty-vt-experimental", default)]
    GhosttyVtExperimental,
}

impl TerminalEngineKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Alacritty => "alacritty",
            #[cfg(feature = "ghostty-vt-experimental")]
            Self::GhosttyVtExperimental => "ghostty-vt-experimental",
        }
    }
}

static DEFAULT_TERMINAL_ENGINE: AtomicU8 = AtomicU8::new(default_terminal_engine_discriminant());

const fn default_terminal_engine_discriminant() -> u8 {
    #[cfg(feature = "ghostty-vt-experimental")]
    {
        1
    }

    #[cfg(not(feature = "ghostty-vt-experimental"))]
    {
        0
    }
}

pub fn default_terminal_engine() -> TerminalEngineKind {
    match DEFAULT_TERMINAL_ENGINE.load(Ordering::Relaxed) {
        0 => TerminalEngineKind::Alacritty,
        #[cfg(feature = "ghostty-vt-experimental")]
        1 => TerminalEngineKind::GhosttyVtExperimental,
        _ => TerminalEngineKind::default(),
    }
}

pub fn set_default_terminal_engine(engine: TerminalEngineKind) {
    DEFAULT_TERMINAL_ENGINE.store(
        match engine {
            TerminalEngineKind::Alacritty => 0,
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEngineKind::GhosttyVtExperimental => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn parse_terminal_engine_kind(value: Option<&str>) -> Result<TerminalEngineKind, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(TerminalEngineKind::default());
    };

    match value.to_ascii_lowercase().as_str() {
        "alacritty" => Ok(TerminalEngineKind::Alacritty),
        "ghostty-vt-experimental" | "ghostty-vt" | "ghostty" => {
            #[cfg(feature = "ghostty-vt-experimental")]
            {
                Ok(TerminalEngineKind::GhosttyVtExperimental)
            }
            #[cfg(not(feature = "ghostty-vt-experimental"))]
            {
                Err(
                    "embedded_terminal_engine `ghostty-vt-experimental` requires Arbor to be built with the `ghostty-vt-experimental` cargo feature"
                        .to_owned(),
                )
            }
        },
        _ => Err(format!(
            "invalid embedded_terminal_engine `{value}`, expected alacritty{}",
            available_terminal_engine_suffix(),
        )),
    }
}

#[cfg(feature = "ghostty-vt-experimental")]
const fn available_terminal_engine_suffix() -> &'static str {
    " or ghostty-vt-experimental"
}

#[cfg(not(feature = "ghostty-vt-experimental"))]
const fn available_terminal_engine_suffix() -> &'static str {
    ""
}

enum TerminalEmulatorInner {
    Alacritty(Box<alacritty_emulator::TerminalEmulator>),
    #[cfg(feature = "ghostty-vt-experimental")]
    Ghostty(ghostty_vt_experimental::TerminalEmulator),
}

pub struct TerminalEmulator {
    inner: TerminalEmulatorInner,
}

impl TerminalEmulator {
    pub fn new() -> Self {
        Self::with_engine(default_terminal_engine(), TERMINAL_ROWS, TERMINAL_COLS)
    }

    pub fn with_size(rows: u16, cols: u16) -> Self {
        Self::with_engine(default_terminal_engine(), rows, cols)
    }

    pub fn with_engine(engine: TerminalEngineKind, rows: u16, cols: u16) -> Self {
        let inner = match engine {
            TerminalEngineKind::Alacritty => TerminalEmulatorInner::Alacritty(Box::new(
                alacritty_emulator::TerminalEmulator::with_size(rows, cols),
            )),
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEngineKind::GhosttyVtExperimental => TerminalEmulatorInner::Ghostty(
                ghostty_vt_experimental::TerminalEmulator::with_size(rows, cols),
            ),
        };
        Self { inner }
    }

    pub fn engine(&self) -> TerminalEngineKind {
        match &self.inner {
            TerminalEmulatorInner::Alacritty(_) => TerminalEngineKind::Alacritty,
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEmulatorInner::Ghostty(_) => TerminalEngineKind::GhosttyVtExperimental,
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        let _ = self.process_and_report(bytes);
    }

    pub fn process_and_report(&mut self, bytes: &[u8]) -> TerminalProcessReport {
        match &mut self.inner {
            TerminalEmulatorInner::Alacritty(emulator) => emulator.process_and_report(bytes),
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEmulatorInner::Ghostty(emulator) => emulator.process_and_report(bytes),
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        match &mut self.inner {
            TerminalEmulatorInner::Alacritty(emulator) => emulator.resize(rows, cols),
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEmulatorInner::Ghostty(emulator) => emulator.resize(rows, cols),
        }
    }

    pub fn snapshot_output(&self) -> String {
        match &self.inner {
            TerminalEmulatorInner::Alacritty(emulator) => emulator.snapshot_output(),
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEmulatorInner::Ghostty(emulator) => emulator.snapshot_output(),
        }
    }

    pub fn snapshot_cursor(&self) -> Option<TerminalCursor> {
        match &self.inner {
            TerminalEmulatorInner::Alacritty(emulator) => emulator.snapshot_cursor(),
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEmulatorInner::Ghostty(emulator) => emulator.snapshot_cursor(),
        }
    }

    pub fn snapshot_modes(&self) -> TerminalModes {
        match &self.inner {
            TerminalEmulatorInner::Alacritty(emulator) => emulator.snapshot_modes(),
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEmulatorInner::Ghostty(emulator) => emulator.snapshot_modes(),
        }
    }

    pub fn collect_styled_lines(&self) -> Vec<TerminalStyledLine> {
        match &self.inner {
            TerminalEmulatorInner::Alacritty(emulator) => emulator.collect_styled_lines(),
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEmulatorInner::Ghostty(emulator) => emulator.collect_styled_lines(),
        }
    }

    pub fn render_ansi_snapshot(&self, max_lines: usize) -> String {
        match &self.inner {
            TerminalEmulatorInner::Alacritty(emulator) => emulator.render_ansi_snapshot(max_lines),
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEmulatorInner::Ghostty(emulator) => emulator.render_ansi_snapshot(max_lines),
        }
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        match &self.inner {
            TerminalEmulatorInner::Alacritty(emulator) => emulator.snapshot(),
            #[cfg(feature = "ghostty-vt-experimental")]
            TerminalEmulatorInner::Ghostty(emulator) => emulator.snapshot(),
        }
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
    fn parse_terminal_engine_defaults_to_expected_engine() {
        #[cfg(feature = "ghostty-vt-experimental")]
        let expected = TerminalEngineKind::GhosttyVtExperimental;
        #[cfg(not(feature = "ghostty-vt-experimental"))]
        let expected = TerminalEngineKind::Alacritty;

        assert_eq!(parse_terminal_engine_kind(None), Ok(expected),);
        assert_eq!(parse_terminal_engine_kind(Some("")), Ok(expected),);
    }

    #[test]
    fn parse_terminal_engine_accepts_alacritty() {
        assert_eq!(
            parse_terminal_engine_kind(Some("alacritty")),
            Ok(TerminalEngineKind::Alacritty),
        );
    }

    #[cfg(feature = "ghostty-vt-experimental")]
    #[test]
    fn parse_terminal_engine_accepts_ghostty() {
        assert_eq!(
            parse_terminal_engine_kind(Some("ghostty-vt-experimental")),
            Ok(TerminalEngineKind::GhosttyVtExperimental),
        );
    }
}
