mod alacritty_support;

#[cfg(not(feature = "ghostty-vt-experimental"))]
mod alacritty_emulator;
#[cfg(feature = "ghostty-vt-experimental")]
mod ghostty_vt_experimental;

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

#[cfg(not(feature = "ghostty-vt-experimental"))]
pub use alacritty_emulator::TerminalEmulator;
pub use alacritty_support::process_terminal_bytes;
#[cfg(feature = "ghostty-vt-experimental")]
pub use ghostty_vt_experimental::TerminalEmulator;
