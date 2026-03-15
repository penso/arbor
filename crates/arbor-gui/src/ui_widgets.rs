use {
    super::*,
    arbor_core::agent::AgentState,
    gpui::{
        AnyElement, Div, ElementId, FontWeight, Image, ImageFormat, Stateful, div, img, px, rgb,
    },
    std::sync::{Arc, OnceLock},
};

pub(crate) fn loading_status_text(theme: ThemePalette, text: impl Into<String>) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme.accent))
        .child(text.into())
}

pub(crate) fn loading_spinner_frame(frame: usize) -> &'static str {
    LOADING_SPINNER_FRAMES[frame % LOADING_SPINNER_FRAMES.len()]
}

pub(crate) fn action_button(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    label: impl Into<String>,
    style: ActionButtonStyle,
    enabled: bool,
) -> Stateful<Div> {
    let background = if enabled && style == ActionButtonStyle::Primary {
        theme.panel_active_bg
    } else {
        theme.panel_bg
    };
    let text_color = if enabled {
        theme.text_primary
    } else {
        theme.text_disabled
    };

    div()
        .id(id)
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
        })
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(background))
        .px_2()
        .py_1()
        .text_xs()
        .text_color(rgb(text_color))
        .child(label.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionButtonStyle {
    Primary,
    Secondary,
}

pub(crate) fn preset_icon_image(kind: AgentPresetKind) -> Arc<Image> {
    static CLAUDE_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static CODEX_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static PI_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static OPENCODE_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static COPILOT_ICON: OnceLock<Arc<Image>> = OnceLock::new();

    let lock = match kind {
        AgentPresetKind::Codex => &CODEX_ICON,
        AgentPresetKind::Claude => &CLAUDE_ICON,
        AgentPresetKind::Pi => &PI_ICON,
        AgentPresetKind::OpenCode => &OPENCODE_ICON,
        AgentPresetKind::Copilot => &COPILOT_ICON,
    };

    lock.get_or_init(|| {
        tracing::info!(
            preset = kind.key(),
            asset = preset_icon_asset_path(kind),
            bytes = preset_icon_bytes(kind).len(),
            "loading preset icon asset"
        );
        Arc::new(Image::from_bytes(
            preset_icon_format(kind),
            preset_icon_bytes(kind).to_vec(),
        ))
    })
    .clone()
}

pub(crate) fn preset_icon_bytes(kind: AgentPresetKind) -> &'static [u8] {
    match kind {
        AgentPresetKind::Codex => PRESET_ICON_CODEX_SVG,
        AgentPresetKind::Claude => PRESET_ICON_CLAUDE_PNG,
        AgentPresetKind::Pi => PRESET_ICON_PI_SVG,
        AgentPresetKind::OpenCode => PRESET_ICON_OPENCODE_SVG,
        AgentPresetKind::Copilot => PRESET_ICON_COPILOT_SVG,
    }
}

pub(crate) fn preset_icon_format(kind: AgentPresetKind) -> ImageFormat {
    match kind {
        AgentPresetKind::Codex
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => ImageFormat::Svg,
        AgentPresetKind::Claude => ImageFormat::Png,
    }
}

pub(crate) fn preset_icon_asset_path(kind: AgentPresetKind) -> &'static str {
    match kind {
        AgentPresetKind::Codex => "assets/preset-icons/codex-white.svg",
        AgentPresetKind::Claude => "assets/preset-icons/claude.png",
        AgentPresetKind::Pi => "assets/preset-icons/pi-white.svg",
        AgentPresetKind::OpenCode => "assets/preset-icons/opencode-white.svg",
        AgentPresetKind::Copilot => "assets/preset-icons/copilot-white.svg",
    }
}

pub(crate) fn log_preset_icon_fallback_once(kind: AgentPresetKind, fallback_glyph: &'static str) {
    static CLAUDE_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static CODEX_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static PI_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static OPENCODE_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static COPILOT_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();

    let once = match kind {
        AgentPresetKind::Codex => &CODEX_FALLBACK_LOGGED,
        AgentPresetKind::Claude => &CLAUDE_FALLBACK_LOGGED,
        AgentPresetKind::Pi => &PI_FALLBACK_LOGGED,
        AgentPresetKind::OpenCode => &OPENCODE_FALLBACK_LOGGED,
        AgentPresetKind::Copilot => &COPILOT_FALLBACK_LOGGED,
    };

    once.get_or_init(|| {
        tracing::warn!(
            preset = kind.key(),
            asset = preset_icon_asset_path(kind),
            bytes = preset_icon_bytes(kind).len(),
            fallback = fallback_glyph,
            "preset icon asset could not be rendered, using fallback glyph"
        );
        eprintln!(
            "WARN preset icon fallback preset={} asset={} bytes={} fallback={}",
            kind.key(),
            preset_icon_asset_path(kind),
            preset_icon_bytes(kind).len(),
            fallback_glyph
        );
    });
}

pub(crate) fn log_preset_icon_render_once(kind: AgentPresetKind) {
    static CLAUDE_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static CODEX_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static PI_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static OPENCODE_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static COPILOT_RENDER_LOGGED: OnceLock<()> = OnceLock::new();

    let once = match kind {
        AgentPresetKind::Codex => &CODEX_RENDER_LOGGED,
        AgentPresetKind::Claude => &CLAUDE_RENDER_LOGGED,
        AgentPresetKind::Pi => &PI_RENDER_LOGGED,
        AgentPresetKind::OpenCode => &OPENCODE_RENDER_LOGGED,
        AgentPresetKind::Copilot => &COPILOT_RENDER_LOGGED,
    };

    once.get_or_init(|| {
        tracing::info!(
            preset = kind.key(),
            asset = preset_icon_asset_path(kind),
            "preset icon render path active"
        );
    });
}

pub(crate) fn preset_icon_render_size_px(kind: AgentPresetKind) -> f32 {
    match kind {
        AgentPresetKind::Codex => 20.,
        AgentPresetKind::Claude
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => 14.,
    }
}

pub(crate) fn agent_preset_button_content(kind: AgentPresetKind, text_color: u32) -> Div {
    log_preset_icon_render_once(kind);
    let icon = preset_icon_image(kind);
    let icon_size = preset_icon_render_size_px(kind);
    // Use consistent slot size for all icons to ensure vertical alignment
    let icon_slot_size = 20_f32;
    let fallback_color = match kind {
        AgentPresetKind::Claude => 0xD97757,
        AgentPresetKind::Codex
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => text_color,
    };
    let fallback_glyph = match kind {
        AgentPresetKind::Claude => "C",
        AgentPresetKind::Codex
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => kind.fallback_icon(),
    };
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .w(px(icon_slot_size))
                .h(px(icon_slot_size))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(img(icon).size(px(icon_size)).with_fallback(move || {
                    log_preset_icon_fallback_once(kind, fallback_glyph);
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(12.))
                        .line_height(px(12.))
                        .text_color(rgb(fallback_color))
                        .child(fallback_glyph)
                        .into_any_element()
                })),
        )
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(14.))
                .text_color(rgb(text_color))
                .child(kind.label()),
        )
}

pub(crate) fn git_action_button(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    icon: &'static str,
    label: &'static str,
    enabled: bool,
    active: bool,
) -> Stateful<Div> {
    let background = if active {
        theme.panel_active_bg
    } else {
        theme.panel_bg
    };
    let icon_color = if active {
        theme.accent
    } else if enabled {
        theme.text_muted
    } else {
        theme.text_disabled
    };
    let text_color = if enabled || active {
        theme.text_primary
    } else {
        theme.text_disabled
    };

    div()
        .id(id)
        .h(px(24.))
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(background))
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
        })
        .child(
            div()
                .font_family(FONT_MONO)
                .text_size(px(13.))
                .text_color(rgb(icon_color))
                .child(icon),
        )
        .child(div().text_xs().text_color(rgb(text_color)).child(label))
}

pub(crate) fn modal_backdrop() -> Div {
    div().absolute().inset_0().bg(rgb(0x000000)).opacity(0.28)
}

pub(crate) fn modal_input_field(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    label: impl Into<String>,
    value: &str,
    cursor: usize,
    placeholder: impl Into<String>,
    active: bool,
) -> Stateful<Div> {
    let label = label.into();
    let placeholder = placeholder.into();

    div()
        .id(id)
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme.text_muted))
                .child(label),
        )
        .child(
            div()
                .overflow_hidden()
                .cursor_pointer()
                .rounded_sm()
                .border_1()
                .border_color(rgb(if active {
                    theme.accent
                } else {
                    theme.border
                }))
                .bg(rgb(theme.panel_bg))
                .px_2()
                .py_1()
                .text_sm()
                .font_family(FONT_MONO)
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(if active {
                    if value.is_empty() {
                        active_input_display(theme, "", &placeholder, theme.text_disabled, 0, 48)
                    } else {
                        active_input_display(
                            theme,
                            value,
                            &placeholder,
                            theme.text_primary,
                            cursor,
                            56,
                        )
                    }
                } else if value.is_empty() {
                    div()
                        .text_color(rgb(theme.text_disabled))
                        .child(placeholder)
                        .into_any_element()
                } else {
                    div()
                        .text_color(rgb(theme.text_primary))
                        .child(value.to_owned())
                        .into_any_element()
                }),
        )
}

pub(crate) fn single_line_input_field(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    value: &str,
    cursor: usize,
    placeholder: impl Into<String>,
    active: bool,
) -> Stateful<Div> {
    let placeholder = placeholder.into();

    div()
        .id(id)
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .h(px(30.))
        .cursor_text()
        .rounded_sm()
        .border_1()
        .border_color(rgb(if active {
            theme.accent
        } else {
            theme.border
        }))
        .bg(rgb(theme.panel_bg))
        .px_2()
        .text_sm()
        .font_family(FONT_MONO)
        .flex()
        .items_center()
        .child(if active {
            if value.is_empty() {
                active_input_display(theme, "", &placeholder, theme.text_disabled, 0, 48)
            } else {
                active_input_display(theme, value, &placeholder, theme.text_primary, cursor, 48)
            }
        } else {
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(rgb(if value.is_empty() {
                    theme.text_disabled
                } else {
                    theme.text_primary
                }))
                .child(if value.is_empty() {
                    placeholder
                } else {
                    value.to_owned()
                })
                .into_any_element()
        })
}

pub(crate) fn active_input_display(
    theme: ThemePalette,
    value: &str,
    placeholder: &str,
    text_color: u32,
    cursor: usize,
    max_chars: usize,
) -> AnyElement {
    if value.is_empty() {
        return div()
            .relative()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(
                div()
                    .text_color(rgb(text_color))
                    .child(placeholder.to_owned()),
            )
            .child(
                input_caret(theme)
                    .flex_none()
                    .absolute()
                    .left(px(0.))
                    .top(px(2.)),
            )
            .into_any_element();
    }

    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .flex()
        .items_center()
        .justify_start()
        .gap(px(0.))
        .child({
            let (before_cursor, after_cursor) = visible_input_segments(value, cursor, max_chars);
            div()
                .flex()
                .items_center()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(text_color))
                        .child(before_cursor),
                )
                .child(input_caret(theme).flex_none())
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(text_color))
                        .child(after_cursor),
                )
        })
        .into_any_element()
}

pub(crate) fn visible_input_segments(
    value: &str,
    cursor: usize,
    max_chars: usize,
) -> (String, String) {
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();
    let cursor = cursor.min(len);
    if len <= max_chars {
        let before: String = chars[..cursor].iter().collect();
        let after: String = chars[cursor..].iter().collect();
        return (before, after);
    }

    let window = max_chars.max(1);
    let preferred_left = window.saturating_sub(8);
    let mut start = cursor.saturating_sub(preferred_left);
    start = start.min(len.saturating_sub(window));
    let end = (start + window).min(len);

    let mut before: String = chars[start..cursor].iter().collect();
    let mut after: String = chars[cursor..end].iter().collect();
    if start > 0 {
        before.insert(0, '\u{2026}');
    }
    if end < len {
        after.push('\u{2026}');
    }
    (before, after)
}

pub(crate) fn input_caret(theme: ThemePalette) -> Div {
    div().w(px(1.)).h(px(14.)).bg(rgb(theme.accent)).mt(px(1.))
}

pub(crate) fn status_text(theme: ThemePalette, text: impl Into<String>) -> Div {
    div()
        .text_xs()
        .text_color(rgb(theme.text_muted))
        .child(text.into())
}

#[derive(Clone, Copy)]
pub(crate) struct WorktreeAttentionIndicator {
    pub(crate) label: &'static str,
    pub(crate) short_label: &'static str,
    pub(crate) color: u32,
}

pub(crate) fn worktree_attention_indicator(
    worktree: &WorktreeSummary,
) -> WorktreeAttentionIndicator {
    if worktree.stuck_turn_count >= 2 {
        return WorktreeAttentionIndicator {
            label: "Stuck",
            short_label: "Stuck",
            color: 0xeb6f92,
        };
    }
    if worktree.agent_state == Some(AgentState::Working) {
        return WorktreeAttentionIndicator {
            label: "Working",
            short_label: "Run",
            color: 0xe5c07b,
        };
    }
    if worktree.agent_state == Some(AgentState::Waiting)
        && worktree
            .recent_turns
            .first()
            .and_then(|snapshot| snapshot.diff_summary)
            .is_some_and(|summary| summary.additions > 0 || summary.deletions > 0)
    {
        return WorktreeAttentionIndicator {
            label: "Needs review",
            short_label: "Review",
            color: 0x61afef,
        };
    }
    if worktree.agent_state == Some(AgentState::Waiting) {
        return WorktreeAttentionIndicator {
            label: "Waiting",
            short_label: "Wait",
            color: 0x61afef,
        };
    }
    if !worktree.detected_ports.is_empty() {
        return WorktreeAttentionIndicator {
            label: "Serving",
            short_label: "Ports",
            color: 0x72d69c,
        };
    }
    if worktree.last_activity_unix_ms.is_some_and(|timestamp| {
        current_unix_timestamp_millis()
            .unwrap_or(0)
            .saturating_sub(timestamp)
            <= 15 * 60 * 1000
    }) {
        return WorktreeAttentionIndicator {
            label: "Recent",
            short_label: "Recent",
            color: 0xc0caf5,
        };
    }

    WorktreeAttentionIndicator {
        label: "Idle",
        short_label: "Idle",
        color: 0x7f8490,
    }
}

pub(crate) fn worktree_activity_sparkline(worktree: &WorktreeSummary) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if worktree.recent_turns.is_empty() {
        return String::new();
    }

    let values: Vec<usize> = worktree
        .recent_turns
        .iter()
        .take(5)
        .rev()
        .map(|snapshot| {
            snapshot
                .diff_summary
                .map(|summary| summary.additions + summary.deletions)
                .unwrap_or(0)
        })
        .collect();
    let max_value = values.iter().copied().max().unwrap_or(0);
    if max_value == 0 {
        return "▁▁▁".to_owned();
    }

    values
        .into_iter()
        .map(|value| {
            let index = value.saturating_mul(BARS.len() - 1) / max_value.max(1);
            BARS[index]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use {super::*, arbor_core::agent::AgentState};

    #[test]
    fn attention_indicator_prefers_stuck_state() {
        let mut worktree = crate::tests::sample_worktree_summary();
        worktree.agent_state = Some(AgentState::Waiting);
        worktree.stuck_turn_count = 2;

        let attention = worktree_attention_indicator(&worktree);
        assert_eq!(attention.label, "Stuck");
    }
}
