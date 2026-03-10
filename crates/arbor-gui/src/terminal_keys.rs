use {crate::terminal_backend::TerminalModes, gpui::Keystroke, std::borrow::Cow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalPlatformCommand {
    Copy,
    Paste,
}

#[derive(Debug, PartialEq, Eq)]
enum TerminalModifiers {
    None,
    Alt,
    Ctrl,
    Shift,
    CtrlShift,
    Other,
}

impl TerminalModifiers {
    fn new(keystroke: &Keystroke) -> Self {
        match (
            keystroke.modifiers.alt,
            keystroke.modifiers.control,
            keystroke.modifiers.shift,
            keystroke.modifiers.platform,
            keystroke.modifiers.function,
        ) {
            (false, false, false, false, false) => Self::None,
            (true, false, false, false, false) => Self::Alt,
            (false, true, false, false, false) => Self::Ctrl,
            (false, false, true, false, false) => Self::Shift,
            (false, true, true, false, false) => Self::CtrlShift,
            _ => Self::Other,
        }
    }

    fn any(&self) -> bool {
        *self != Self::None
    }
}

pub fn platform_command_for_keystroke(keystroke: &Keystroke) -> Option<TerminalPlatformCommand> {
    // macOS: Cmd+C/V
    #[cfg(target_os = "macos")]
    {
        if !keystroke.modifiers.platform {
            return None;
        }

        if keystroke.modifiers.control || keystroke.modifiers.alt || keystroke.modifiers.function {
            return None;
        }

        if keystroke.key.eq_ignore_ascii_case("c") {
            return Some(TerminalPlatformCommand::Copy);
        }
        if keystroke.key.eq_ignore_ascii_case("v") {
            return Some(TerminalPlatformCommand::Paste);
        }
    }

    // Linux/other: Ctrl+Shift+C/V
    #[cfg(not(target_os = "macos"))]
    {
        if !keystroke.modifiers.control || !keystroke.modifiers.shift {
            return None;
        }

        if keystroke.modifiers.alt || keystroke.modifiers.function || keystroke.modifiers.platform {
            return None;
        }

        if keystroke.key.eq_ignore_ascii_case("c") {
            return Some(TerminalPlatformCommand::Copy);
        }
        if keystroke.key.eq_ignore_ascii_case("v") {
            return Some(TerminalPlatformCommand::Paste);
        }
    }

    None
}

pub fn terminal_bytes_from_keystroke(
    keystroke: &Keystroke,
    modes: TerminalModes,
) -> Option<Vec<u8>> {
    to_esc_str(keystroke, modes).map(|value| value.into_owned().into_bytes())
}

fn to_esc_str(keystroke: &Keystroke, modes: TerminalModes) -> Option<Cow<'static, str>> {
    if keystroke.modifiers.platform {
        return None;
    }

    // On Linux, Ctrl+Shift+C/V are copy/paste - don't send to terminal
    #[cfg(not(target_os = "macos"))]
    if keystroke.modifiers.control
        && keystroke.modifiers.shift
        && (keystroke.key.eq_ignore_ascii_case("c") || keystroke.key.eq_ignore_ascii_case("v"))
    {
        return None;
    }

    let modifiers = TerminalModifiers::new(keystroke);
    let key = normalized_key(keystroke);

    let manual_esc_str: Option<&'static str> = match (key, &modifiers) {
        ("tab", TerminalModifiers::None) => Some("\x09"),
        ("escape", TerminalModifiers::None) => Some("\x1b"),
        ("enter", TerminalModifiers::None) => Some("\x0d"),
        ("enter", TerminalModifiers::Shift) => Some("\x0a"),
        ("enter", TerminalModifiers::Alt) => Some("\x1b\x0d"),
        ("backspace", TerminalModifiers::None) => Some("\x7f"),
        ("tab", TerminalModifiers::Shift) => Some("\x1b[Z"),
        ("backspace", TerminalModifiers::Ctrl) => Some("\x08"),
        ("backspace", TerminalModifiers::Alt) => Some("\x1b\x7f"),
        ("backspace", TerminalModifiers::Shift) => Some("\x7f"),
        ("space", TerminalModifiers::Ctrl) => Some("\x00"),
        ("home", TerminalModifiers::Shift) if modes.alt_screen => Some("\x1b[1;2H"),
        ("end", TerminalModifiers::Shift) if modes.alt_screen => Some("\x1b[1;2F"),
        ("pageup", TerminalModifiers::Shift) if modes.alt_screen => Some("\x1b[5;2~"),
        ("pagedown", TerminalModifiers::Shift) if modes.alt_screen => Some("\x1b[6;2~"),
        ("home", TerminalModifiers::None) if modes.app_cursor => Some("\x1bOH"),
        ("home", TerminalModifiers::None) => Some("\x1b[H"),
        ("end", TerminalModifiers::None) if modes.app_cursor => Some("\x1bOF"),
        ("end", TerminalModifiers::None) => Some("\x1b[F"),
        ("up", TerminalModifiers::None) if modes.app_cursor => Some("\x1bOA"),
        ("up", TerminalModifiers::None) => Some("\x1b[A"),
        ("down", TerminalModifiers::None) if modes.app_cursor => Some("\x1bOB"),
        ("down", TerminalModifiers::None) => Some("\x1b[B"),
        ("right", TerminalModifiers::None) if modes.app_cursor => Some("\x1bOC"),
        ("right", TerminalModifiers::None) => Some("\x1b[C"),
        ("left", TerminalModifiers::None) if modes.app_cursor => Some("\x1bOD"),
        ("left", TerminalModifiers::None) => Some("\x1b[D"),
        ("back", TerminalModifiers::None) => Some("\x7f"),
        ("insert", TerminalModifiers::None) => Some("\x1b[2~"),
        ("delete", TerminalModifiers::None) => Some("\x1b[3~"),
        ("pageup", TerminalModifiers::None) => Some("\x1b[5~"),
        ("pagedown", TerminalModifiers::None) => Some("\x1b[6~"),
        ("f1", TerminalModifiers::None) => Some("\x1bOP"),
        ("f2", TerminalModifiers::None) => Some("\x1bOQ"),
        ("f3", TerminalModifiers::None) => Some("\x1bOR"),
        ("f4", TerminalModifiers::None) => Some("\x1bOS"),
        ("f5", TerminalModifiers::None) => Some("\x1b[15~"),
        ("f6", TerminalModifiers::None) => Some("\x1b[17~"),
        ("f7", TerminalModifiers::None) => Some("\x1b[18~"),
        ("f8", TerminalModifiers::None) => Some("\x1b[19~"),
        ("f9", TerminalModifiers::None) => Some("\x1b[20~"),
        ("f10", TerminalModifiers::None) => Some("\x1b[21~"),
        ("f11", TerminalModifiers::None) => Some("\x1b[23~"),
        ("f12", TerminalModifiers::None) => Some("\x1b[24~"),
        ("f13", TerminalModifiers::None) => Some("\x1b[25~"),
        ("f14", TerminalModifiers::None) => Some("\x1b[26~"),
        ("f15", TerminalModifiers::None) => Some("\x1b[28~"),
        ("f16", TerminalModifiers::None) => Some("\x1b[29~"),
        ("f17", TerminalModifiers::None) => Some("\x1b[31~"),
        ("f18", TerminalModifiers::None) => Some("\x1b[32~"),
        ("f19", TerminalModifiers::None) => Some("\x1b[33~"),
        ("f20", TerminalModifiers::None) => Some("\x1b[34~"),
        ("a", TerminalModifiers::Ctrl) | ("A", TerminalModifiers::CtrlShift) => Some("\x01"),
        ("b", TerminalModifiers::Ctrl) | ("B", TerminalModifiers::CtrlShift) => Some("\x02"),
        ("c", TerminalModifiers::Ctrl) | ("C", TerminalModifiers::CtrlShift) => Some("\x03"),
        ("d", TerminalModifiers::Ctrl) | ("D", TerminalModifiers::CtrlShift) => Some("\x04"),
        ("e", TerminalModifiers::Ctrl) | ("E", TerminalModifiers::CtrlShift) => Some("\x05"),
        ("f", TerminalModifiers::Ctrl) | ("F", TerminalModifiers::CtrlShift) => Some("\x06"),
        ("g", TerminalModifiers::Ctrl) | ("G", TerminalModifiers::CtrlShift) => Some("\x07"),
        ("h", TerminalModifiers::Ctrl) | ("H", TerminalModifiers::CtrlShift) => Some("\x08"),
        ("i", TerminalModifiers::Ctrl) | ("I", TerminalModifiers::CtrlShift) => Some("\x09"),
        ("j", TerminalModifiers::Ctrl) | ("J", TerminalModifiers::CtrlShift) => Some("\x0a"),
        ("k", TerminalModifiers::Ctrl) | ("K", TerminalModifiers::CtrlShift) => Some("\x0b"),
        ("l", TerminalModifiers::Ctrl) | ("L", TerminalModifiers::CtrlShift) => Some("\x0c"),
        ("m", TerminalModifiers::Ctrl) | ("M", TerminalModifiers::CtrlShift) => Some("\x0d"),
        ("n", TerminalModifiers::Ctrl) | ("N", TerminalModifiers::CtrlShift) => Some("\x0e"),
        ("o", TerminalModifiers::Ctrl) | ("O", TerminalModifiers::CtrlShift) => Some("\x0f"),
        ("p", TerminalModifiers::Ctrl) | ("P", TerminalModifiers::CtrlShift) => Some("\x10"),
        ("q", TerminalModifiers::Ctrl) | ("Q", TerminalModifiers::CtrlShift) => Some("\x11"),
        ("r", TerminalModifiers::Ctrl) | ("R", TerminalModifiers::CtrlShift) => Some("\x12"),
        ("s", TerminalModifiers::Ctrl) | ("S", TerminalModifiers::CtrlShift) => Some("\x13"),
        ("t", TerminalModifiers::Ctrl) | ("T", TerminalModifiers::CtrlShift) => Some("\x14"),
        ("u", TerminalModifiers::Ctrl) | ("U", TerminalModifiers::CtrlShift) => Some("\x15"),
        ("v", TerminalModifiers::Ctrl) | ("V", TerminalModifiers::CtrlShift) => Some("\x16"),
        ("w", TerminalModifiers::Ctrl) | ("W", TerminalModifiers::CtrlShift) => Some("\x17"),
        ("x", TerminalModifiers::Ctrl) | ("X", TerminalModifiers::CtrlShift) => Some("\x18"),
        ("y", TerminalModifiers::Ctrl) | ("Y", TerminalModifiers::CtrlShift) => Some("\x19"),
        ("z", TerminalModifiers::Ctrl) | ("Z", TerminalModifiers::CtrlShift) => Some("\x1a"),
        ("@", TerminalModifiers::Ctrl) => Some("\x00"),
        ("[", TerminalModifiers::Ctrl) => Some("\x1b"),
        ("\\", TerminalModifiers::Ctrl) => Some("\x1c"),
        ("]", TerminalModifiers::Ctrl) => Some("\x1d"),
        ("^", TerminalModifiers::Ctrl) => Some("\x1e"),
        ("_", TerminalModifiers::Ctrl) => Some("\x1f"),
        ("?", TerminalModifiers::Ctrl) => Some("\x7f"),
        _ => None,
    };
    if let Some(esc_str) = manual_esc_str {
        return Some(Cow::Borrowed(esc_str));
    }

    if modifiers.any() {
        let modifier_code = modifier_code(keystroke);
        let modified_esc_str = match key {
            "up" => Some(format!("\x1b[1;{modifier_code}A")),
            "down" => Some(format!("\x1b[1;{modifier_code}B")),
            "right" => Some(format!("\x1b[1;{modifier_code}C")),
            "left" => Some(format!("\x1b[1;{modifier_code}D")),
            "f1" => Some(format!("\x1b[1;{modifier_code}P")),
            "f2" => Some(format!("\x1b[1;{modifier_code}Q")),
            "f3" => Some(format!("\x1b[1;{modifier_code}R")),
            "f4" => Some(format!("\x1b[1;{modifier_code}S")),
            "f5" => Some(format!("\x1b[15;{modifier_code}~")),
            "f6" => Some(format!("\x1b[17;{modifier_code}~")),
            "f7" => Some(format!("\x1b[18;{modifier_code}~")),
            "f8" => Some(format!("\x1b[19;{modifier_code}~")),
            "f9" => Some(format!("\x1b[20;{modifier_code}~")),
            "f10" => Some(format!("\x1b[21;{modifier_code}~")),
            "f11" => Some(format!("\x1b[23;{modifier_code}~")),
            "f12" => Some(format!("\x1b[24;{modifier_code}~")),
            "f13" => Some(format!("\x1b[25;{modifier_code}~")),
            "f14" => Some(format!("\x1b[26;{modifier_code}~")),
            "f15" => Some(format!("\x1b[28;{modifier_code}~")),
            "f16" => Some(format!("\x1b[29;{modifier_code}~")),
            "f17" => Some(format!("\x1b[31;{modifier_code}~")),
            "f18" => Some(format!("\x1b[32;{modifier_code}~")),
            "f19" => Some(format!("\x1b[33;{modifier_code}~")),
            "f20" => Some(format!("\x1b[34;{modifier_code}~")),
            _ if modifier_code == 2 => None,
            "insert" => Some(format!("\x1b[2;{modifier_code}~")),
            "pageup" => Some(format!("\x1b[5;{modifier_code}~")),
            "pagedown" => Some(format!("\x1b[6;{modifier_code}~")),
            "end" => Some(format!("\x1b[1;{modifier_code}F")),
            "home" => Some(format!("\x1b[1;{modifier_code}H")),
            _ => None,
        };
        if let Some(esc_str) = modified_esc_str {
            return Some(Cow::Owned(esc_str));
        }
    }

    if !cfg!(target_os = "macos") {
        let is_alt_lowercase_ascii =
            modifiers == TerminalModifiers::Alt && keystroke.key.is_ascii();
        let is_alt_uppercase_ascii =
            keystroke.modifiers.alt && keystroke.modifiers.shift && keystroke.key.is_ascii();
        if is_alt_lowercase_ascii || is_alt_uppercase_ascii {
            let key = if is_alt_uppercase_ascii {
                keystroke.key.to_ascii_uppercase()
            } else {
                keystroke.key.clone()
            };
            return Some(Cow::Owned(format!("\x1b{key}")));
        }
    }

    None
}

fn normalized_key(keystroke: &Keystroke) -> &str {
    match keystroke.key.as_str() {
        "return" => "enter",
        key => key,
    }
}

fn modifier_code(keystroke: &Keystroke) -> u32 {
    let mut modifier_code = 0;
    if keystroke.modifiers.shift {
        modifier_code |= 1;
    }
    if keystroke.modifiers.alt {
        modifier_code |= 1 << 1;
    }
    if keystroke.modifiers.control {
        modifier_code |= 1 << 2;
    }
    modifier_code + 1
}

#[cfg(test)]
mod tests {
    use {
        crate::{
            terminal_backend::TerminalModes,
            terminal_keys::{
                TerminalPlatformCommand, modifier_code, platform_command_for_keystroke,
                terminal_bytes_from_keystroke,
            },
        },
        gpui::{Keystroke, Modifiers},
    };

    fn parse_keystroke(source: &str) -> Keystroke {
        match Keystroke::parse(source) {
            Ok(keystroke) => keystroke,
            Err(error) => panic!("invalid test keystroke `{source}`: {error}"),
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn maps_platform_copy_and_paste_macos() {
        let copy = parse_keystroke("cmd-c");
        let paste = parse_keystroke("cmd-v");

        assert_eq!(
            platform_command_for_keystroke(&copy),
            Some(TerminalPlatformCommand::Copy)
        );
        assert_eq!(
            platform_command_for_keystroke(&paste),
            Some(TerminalPlatformCommand::Paste)
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn maps_platform_copy_and_paste_linux() {
        let copy = parse_keystroke("ctrl-shift-c");
        let paste = parse_keystroke("ctrl-shift-v");

        assert_eq!(
            platform_command_for_keystroke(&copy),
            Some(TerminalPlatformCommand::Copy)
        );
        assert_eq!(
            platform_command_for_keystroke(&paste),
            Some(TerminalPlatformCommand::Paste)
        );
    }

    #[test]
    fn does_not_treat_control_c_as_platform_copy() {
        let control_c = parse_keystroke("ctrl-c");
        assert_eq!(platform_command_for_keystroke(&control_c), None);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn ctrl_shift_c_does_not_send_to_terminal_on_linux() {
        let ctrl_shift_c = parse_keystroke("ctrl-shift-c");
        assert_eq!(
            terminal_bytes_from_keystroke(&ctrl_shift_c, TerminalModes::default()),
            None
        );
    }

    #[test]
    fn maps_control_c_to_interrupt_byte() {
        let control_c = parse_keystroke("ctrl-c");
        assert_eq!(
            terminal_bytes_from_keystroke(&control_c, TerminalModes::default()),
            Some(vec![0x03])
        );
    }

    #[test]
    fn plain_text_returns_none_for_ime_path() {
        let plain_a = parse_keystroke("a");
        assert_eq!(
            terminal_bytes_from_keystroke(&plain_a, TerminalModes::default()),
            None
        );
    }

    #[test]
    fn maps_shift_tab_to_backtab_escape_sequence() {
        let shift_tab = parse_keystroke("shift-tab");
        assert_eq!(
            terminal_bytes_from_keystroke(&shift_tab, TerminalModes::default()),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn ignores_platform_shortcuts_for_terminal_bytes() {
        let command_v = parse_keystroke("cmd-v");
        assert_eq!(
            terminal_bytes_from_keystroke(&command_v, TerminalModes::default()),
            None
        );
    }

    #[test]
    fn shift_enter_sends_line_feed() {
        let shift_enter = parse_keystroke("shift-enter");
        let enter = parse_keystroke("enter");

        assert_eq!(
            terminal_bytes_from_keystroke(&shift_enter, TerminalModes::default()),
            Some(vec![b'\n'])
        );
        assert_eq!(
            terminal_bytes_from_keystroke(&enter, TerminalModes::default()),
            Some(vec![b'\r'])
        );
    }

    #[test]
    fn arrow_keys_follow_application_cursor_mode() {
        let up = parse_keystroke("up");

        assert_eq!(
            terminal_bytes_from_keystroke(&up, TerminalModes::default()),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            terminal_bytes_from_keystroke(&up, TerminalModes {
                app_cursor: true,
                alt_screen: false,
            }),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn shift_navigation_uses_alt_screen_sequences() {
        let shift_pageup = parse_keystroke("shift-pageup");

        assert_eq!(
            terminal_bytes_from_keystroke(&shift_pageup, TerminalModes::default()),
            None
        );
        assert_eq!(
            terminal_bytes_from_keystroke(&shift_pageup, TerminalModes {
                app_cursor: false,
                alt_screen: true,
            }),
            Some(b"\x1b[5;2~".to_vec())
        );
    }

    #[test]
    fn modifier_code_matches_xterm_convention() {
        assert_eq!(2, modifier_code(&parse_keystroke("shift-a")));
        assert_eq!(3, modifier_code(&parse_keystroke("alt-a")));
        assert_eq!(4, modifier_code(&parse_keystroke("shift-alt-a")));
        assert_eq!(5, modifier_code(&parse_keystroke("ctrl-a")));
        assert_eq!(6, modifier_code(&parse_keystroke("shift-ctrl-a")));
        assert_eq!(7, modifier_code(&parse_keystroke("alt-ctrl-a")));
        assert_eq!(8, modifier_code(&parse_keystroke("shift-ctrl-alt-a")));
    }

    #[test]
    fn plain_multibyte_keys_still_use_text_input_path() {
        let ks = Keystroke {
            modifiers: Modifiers {
                control: false,
                alt: false,
                shift: false,
                platform: false,
                function: false,
            },
            key: "🖖🏻".to_string(),
            key_char: None,
        };

        assert_eq!(
            terminal_bytes_from_keystroke(&ks, TerminalModes::default()),
            None
        );
    }
}
