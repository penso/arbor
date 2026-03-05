#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    One,
    Ayu,
    Gruvbox,
}

#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub chrome_bg: u32,
    pub chrome_border: u32,
    pub app_bg: u32,
    pub sidebar_bg: u32,
    pub terminal_bg: u32,
    pub panel_bg: u32,
    pub panel_active_bg: u32,
    pub tab_bg: u32,
    pub tab_active_bg: u32,
    pub border: u32,
    pub text_primary: u32,
    pub text_muted: u32,
    pub text_disabled: u32,
    pub notice_bg: u32,
    pub notice_text: u32,
    pub accent: u32,
    pub terminal_cursor: u32,
    pub terminal_selection_bg: u32,
    pub terminal_selection_fg: u32,
}

impl ThemeKind {
    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::One => "One Dark",
            ThemeKind::Ayu => "Ayu Dark",
            ThemeKind::Gruvbox => "Gruvbox Dark",
        }
    }

    pub fn palette(self) -> ThemePalette {
        match self {
            ThemeKind::One => ThemePalette {
                chrome_bg: 0x3b414d,
                chrome_border: 0x464b57,
                app_bg: 0x282c33,
                sidebar_bg: 0x2f343e,
                terminal_bg: 0x282c34,
                panel_bg: 0x2e343e,
                panel_active_bg: 0x363c46,
                tab_bg: 0x2f343e,
                tab_active_bg: 0x282c33,
                border: 0x363c46,
                text_primary: 0xc8ccd4,
                text_muted: 0x838994,
                text_disabled: 0x696b77,
                notice_bg: 0x5d3330,
                notice_text: 0xffe3dc,
                accent: 0x74ade8,
                terminal_cursor: 0xebdbb2,
                terminal_selection_bg: 0x3e4451,
                terminal_selection_fg: 0xc8ccd4,
            },
            ThemeKind::Ayu => ThemePalette {
                chrome_bg: 0x313337,
                chrome_border: 0x3f4043,
                app_bg: 0x0d1016,
                sidebar_bg: 0x1f2127,
                terminal_bg: 0x0d1016,
                panel_bg: 0x1f2127,
                panel_active_bg: 0x2a2d34,
                tab_bg: 0x1f2127,
                tab_active_bg: 0x0d1016,
                border: 0x3f4043,
                text_primary: 0xbfbdb6,
                text_muted: 0x8a8986,
                text_disabled: 0x6f6e6b,
                notice_bg: 0x5d3330,
                notice_text: 0xffe3dc,
                accent: 0x5ac1fe,
                terminal_cursor: 0xbfbdb6,
                terminal_selection_bg: 0x2f3f53,
                terminal_selection_fg: 0xd9d7cf,
            },
            ThemeKind::Gruvbox => ThemePalette {
                chrome_bg: 0x3c3836,
                chrome_border: 0x504945,
                app_bg: 0x282828,
                sidebar_bg: 0x32302f,
                terminal_bg: 0x282828,
                panel_bg: 0x32302f,
                panel_active_bg: 0x3c3836,
                tab_bg: 0x32302f,
                tab_active_bg: 0x282828,
                border: 0x504945,
                text_primary: 0xebdbb2,
                text_muted: 0xa89984,
                text_disabled: 0x928374,
                notice_bg: 0x5d3330,
                notice_text: 0xffe3dc,
                accent: 0x83a598,
                terminal_cursor: 0xebdbb2,
                terminal_selection_bg: 0x504945,
                terminal_selection_fg: 0xebdbb2,
            },
        }
    }
}
