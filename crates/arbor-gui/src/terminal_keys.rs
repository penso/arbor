use gpui::Keystroke;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalPlatformCommand {
    Copy,
    Paste,
}

pub fn platform_command_for_keystroke(keystroke: &Keystroke) -> Option<TerminalPlatformCommand> {
    if !keystroke.modifiers.platform {
        return None;
    }

    if keystroke.modifiers.control || keystroke.modifiers.alt {
        return None;
    }

    if keystroke.key.eq_ignore_ascii_case("c") {
        return Some(TerminalPlatformCommand::Copy);
    }
    if keystroke.key.eq_ignore_ascii_case("v") {
        return Some(TerminalPlatformCommand::Paste);
    }

    None
}

pub fn terminal_bytes_from_keystroke(keystroke: &Keystroke) -> Option<Vec<u8>> {
    if keystroke.modifiers.platform {
        return None;
    }

    let key = keystroke.key.as_str();

    if keystroke.modifiers.control {
        if key.len() == 1 {
            let byte = key.as_bytes().first().copied()?;
            let lower = byte.to_ascii_lowercase();
            if lower.is_ascii_lowercase() {
                return Some(vec![lower & 0x1f]);
            }
        }

        if key == "space" {
            return Some(vec![0]);
        }
    }

    match key {
        "enter" | "return" => Some(vec![b'\r']),
        "tab" if keystroke.modifiers.shift => Some(b"\x1b[Z".to_vec()),
        "tab" => Some(vec![b'\t']),
        "backspace" => Some(vec![0x7f]),
        "escape" => Some(vec![0x1b]),
        "up" => Some(b"\x1b[A".to_vec()),
        "down" => Some(b"\x1b[B".to_vec()),
        "right" => Some(b"\x1b[C".to_vec()),
        "left" => Some(b"\x1b[D".to_vec()),
        "home" => Some(b"\x1b[H".to_vec()),
        "end" => Some(b"\x1b[F".to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        "delete" => Some(b"\x1b[3~".to_vec()),
        _ => {
            if !keystroke.modifiers.control
                && !keystroke.modifiers.alt
                && let Some(key_char) = keystroke.key_char.as_ref()
            {
                return Some(key_char.as_bytes().to_vec());
            }

            if !keystroke.modifiers.control && !keystroke.modifiers.alt && key.len() == 1 {
                return Some(key.as_bytes().to_vec());
            }

            None
        },
    }
}

#[cfg(test)]
mod tests {
    use {
        crate::terminal_keys::{
            TerminalPlatformCommand, platform_command_for_keystroke, terminal_bytes_from_keystroke,
        },
        gpui::Keystroke,
    };

    fn parse_keystroke(source: &str) -> Keystroke {
        match Keystroke::parse(source) {
            Ok(keystroke) => keystroke,
            Err(error) => panic!("invalid test keystroke `{source}`: {error}"),
        }
    }

    #[test]
    fn maps_platform_copy_and_paste() {
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
    fn does_not_treat_control_c_as_platform_copy() {
        let control_c = parse_keystroke("ctrl-c");
        assert_eq!(platform_command_for_keystroke(&control_c), None);
    }

    #[test]
    fn maps_control_c_to_interrupt_byte() {
        let control_c = parse_keystroke("ctrl-c");
        assert_eq!(terminal_bytes_from_keystroke(&control_c), Some(vec![0x03]));
    }

    #[test]
    fn maps_plain_text_to_input_bytes() {
        let plain_a = parse_keystroke("a");
        assert_eq!(terminal_bytes_from_keystroke(&plain_a), Some(vec![b'a']));
    }

    #[test]
    fn maps_shift_tab_to_backtab_escape_sequence() {
        let shift_tab = parse_keystroke("shift-tab");
        assert_eq!(
            terminal_bytes_from_keystroke(&shift_tab),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn ignores_platform_shortcuts_for_terminal_bytes() {
        let command_v = parse_keystroke("cmd-v");
        assert_eq!(terminal_bytes_from_keystroke(&command_v), None);
    }
}
