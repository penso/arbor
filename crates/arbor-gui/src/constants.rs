use {
    gpui::{
        App, FontFallbacks, FontFeatures, SharedString, TitlebarOptions, WindowDecorations, font,
    },
    std::{
        borrow::Cow,
        env, fs,
        path::{Path, PathBuf},
        time::Duration,
    },
};

pub(crate) const APP_VERSION: &str = match option_env!("ARBOR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
pub(crate) const APP_NAME: &str = "Arbor";
// Provided by the build environment. The repo's `just` GUI/build recipes set
// this automatically so the title can include the branch without a build script.
pub(crate) const APP_BUILD_BRANCH: Option<&str> = match option_env!("ARBOR_BUILD_BRANCH") {
    Some(branch) if !branch.is_empty() => Some(branch),
    _ => None,
};

pub(crate) const FONT_UI: &str = ".ZedSans";
pub(crate) const FONT_MONO: &str = "CaskaydiaMono Nerd Font Mono";
#[cfg(target_os = "macos")]
pub(crate) const TERMINAL_FONT_FAMILIES: [&str; 5] =
    [FONT_MONO, "SF Mono", "Menlo", "Monaco", "Courier New"];
#[cfg(not(target_os = "macos"))]
pub(crate) const TERMINAL_FONT_FAMILIES: [&str; 6] = [
    FONT_MONO,
    ".ZedMono",
    "SF Mono",
    "Menlo",
    "Monaco",
    "Courier New",
];
pub(crate) const TERMINAL_CELL_WIDTH_PX: f32 = 9.0;
pub(crate) const TERMINAL_CELL_HEIGHT_PX: f32 = 19.0;
pub(crate) const TERMINAL_FONT_SIZE_PX: f32 = 15.0;
pub(crate) const TERMINAL_SCROLLBAR_WIDTH_PX: f32 = 12.0;

pub(crate) const TITLEBAR_HEIGHT: f32 = 34.;

// Left offset for top bar controls. macOS needs space to clear traffic lights,
// Linux uses a smaller offset since window controls are on the right or
// handled by server-side decorations.
#[cfg(target_os = "macos")]
pub(crate) const TOP_BAR_LEFT_OFFSET: f32 = 76.;
#[cfg(not(target_os = "macos"))]
pub(crate) const TOP_BAR_LEFT_OFFSET: f32 = 8.;

#[cfg(target_os = "linux")]
pub(crate) const DEFAULT_WINDOW_DECORATIONS: WindowDecorations = WindowDecorations::Server;
#[cfg(not(target_os = "linux"))]
pub(crate) const DEFAULT_WINDOW_DECORATIONS: WindowDecorations = WindowDecorations::Client;

/// Platform-aware titlebar options. On macOS, uses a transparent titlebar with
/// custom traffic-light positioning. On Linux (server-side decorations), these
/// macOS-specific options are omitted so the compositor can provide native
/// window controls, drag, and resize.
pub(crate) fn default_titlebar_options(title: Option<SharedString>) -> TitlebarOptions {
    #[cfg(target_os = "macos")]
    use gpui::{point, px};

    let title = title.unwrap_or_else(|| app_window_title(None).into());
    TitlebarOptions {
        title: Some(title),
        #[cfg(target_os = "macos")]
        appears_transparent: true,
        #[cfg(not(target_os = "macos"))]
        appears_transparent: false,
        #[cfg(target_os = "macos")]
        traffic_light_position: Some(point(px(9.), px(9.))),
        #[cfg(not(target_os = "macos"))]
        traffic_light_position: None,
    }
}

pub(crate) const WORKTREE_AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
pub(crate) const GITHUB_PR_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
pub(crate) const GITHUB_PR_REFRESH_CONCURRENCY: usize = 4;
pub(crate) const GITHUB_PR_REFRESH_WORKER_STAGGER: Duration = Duration::from_millis(75);
pub(crate) const LOADING_SPINNER_FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
pub(crate) const GITHUB_DEVICE_FLOW_POLL_MIN_INTERVAL: Duration = Duration::from_secs(5);
pub(crate) const GITHUB_OAUTH_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub(crate) const GITHUB_OAUTH_ACCESS_TOKEN_URL: &str =
    "https://github.com/login/oauth/access_token";
pub(crate) const GITHUB_OAUTH_SCOPE: &str = "repo read:user";
pub(crate) const BUILT_IN_GITHUB_OAUTH_CLIENT_ID: Option<&str> = Some("Ov23liVexfjFZQXcuQib");
pub(crate) const GITHUB_AUTH_COPY_FEEDBACK_DURATION: Duration = Duration::from_millis(1200);
pub(crate) const CONFIG_AUTO_REFRESH_INTERVAL: Duration = Duration::from_millis(600);
pub(crate) const TERMINAL_TAB_COMMAND_MAX_CHARS: usize = 14;
pub(crate) const ACTIVE_DAEMON_TERMINAL_SYNC_INTERVAL: Duration = Duration::from_millis(250);
pub(crate) const INACTIVE_DAEMON_TERMINAL_SYNC_INTERVAL: Duration = Duration::from_millis(250);
pub(crate) const IDLE_DAEMON_TERMINAL_SYNC_INTERVAL: Duration = Duration::from_millis(1000);
pub(crate) const DAEMON_TERMINAL_WS_RECONNECT_BASE_DELAY: Duration = Duration::from_millis(150);
pub(crate) const DAEMON_TERMINAL_WS_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(2);
pub(crate) const DEFAULT_DAEMON_BASE_URL: &str = "http://127.0.0.1:8787";
pub(crate) const DEFAULT_DAEMON_PORT: u16 = 8787;
pub(crate) const DEFAULT_SSH_PORT: u16 = 22;
pub(crate) const DEFAULT_LEFT_PANE_WIDTH: f32 = 290.;
pub(crate) const DEFAULT_RIGHT_PANE_WIDTH: f32 = 340.;
pub(crate) const LEFT_PANE_MIN_WIDTH: f32 = 220.;
pub(crate) const LEFT_PANE_MAX_WIDTH: f32 = 520.;
pub(crate) const RIGHT_PANE_MIN_WIDTH: f32 = 240.;
pub(crate) const RIGHT_PANE_MAX_WIDTH: f32 = 560.;
pub(crate) const PANE_RESIZE_HANDLE_WIDTH: f32 = 8.;
pub(crate) const PANE_CENTER_MIN_WIDTH: f32 = 360.;
pub(crate) const DIFF_ROW_HEIGHT_PX: f32 = 19.;
pub(crate) const DIFF_LINE_NUMBER_WIDTH_CHARS: usize = 5;
pub(crate) const DIFF_ZONEMAP_WIDTH_PX: f32 = 14.;
pub(crate) const DIFF_ZONEMAP_MARGIN_PX: f32 = 4.;
pub(crate) const DIFF_ZONEMAP_MARKER_HEIGHT_PX: f32 = 2.;
pub(crate) const DIFF_ZONEMAP_MIN_THUMB_HEIGHT_PX: f32 = 12.;
pub(crate) const DIFF_FONT_SIZE_PX: f32 = 12.0;
pub(crate) const DIFF_HUNK_CONTEXT_LINES: usize = 3;

pub(crate) const TAB_ICON_DIFF: &str = "\u{f440}";
pub(crate) const TAB_ICON_FILE: &str = "\u{f15c}";
pub(crate) const GIT_ACTION_ICON_COMMIT: &str = "\u{f417}";
pub(crate) const GIT_ACTION_ICON_PUSH: &str = "\u{f093}";
pub(crate) const GIT_ACTION_ICON_PR: &str = "\u{f126}";
pub(crate) const COMMAND_PALETTE_MAX_HEIGHT_PX: f32 = 360.;
pub(crate) const COMMAND_PALETTE_ROW_ESTIMATE_PX: f32 = 52.;
pub(crate) const COMMAND_PALETTE_SCROLLBAR_TRACK_HEIGHT_PX: f32 = 336.;
pub(crate) const LOG_POLLER_INTERVAL: Duration = Duration::from_millis(200);
pub(crate) const MEMORY_POLLER_INTERVAL: Duration = Duration::from_secs(5);
pub(crate) const THEME_TOAST_DURATION: Duration = Duration::from_millis(1600);
pub(crate) const WORKTREE_HOVER_POPOVER_HIDE_DELAY: Duration = Duration::from_millis(300);
pub(crate) const WORKTREE_HOVER_POPOVER_CARD_WIDTH_PX: f32 = 300.;
pub(crate) const WORKTREE_HOVER_POPOVER_ZONE_PADDING_PX: f32 = 12.;
pub(crate) const WORKTREE_HOVER_TRIGGER_ZONE_HEIGHT_PX: f32 = 44.;
pub(crate) const PRESET_ICON_CLAUDE_PNG: &[u8] =
    include_bytes!("../../../assets/preset-icons/claude.png");
pub(crate) const PRESET_ICON_CODEX_SVG: &[u8] =
    include_bytes!("../../../assets/preset-icons/codex-white.svg");
pub(crate) const PRESET_ICON_PI_SVG: &[u8] =
    include_bytes!("../../../assets/preset-icons/pi-white.svg");
pub(crate) const PRESET_ICON_OPENCODE_SVG: &[u8] =
    include_bytes!("../../../assets/preset-icons/opencode-white.svg");
pub(crate) const PRESET_ICON_COPILOT_SVG: &[u8] =
    include_bytes!("../../../assets/preset-icons/copilot-white.svg");

pub(crate) const BUNDLED_FONT_FILES: &[&str] = &[
    "CaskaydiaMonoNerdFontMono-Regular.ttf",
    "CaskaydiaMonoNerdFontMono-Bold.ttf",
    "IBMPlexSans-Regular.ttf",
    "IBMPlexSans-Bold.ttf",
    "IBMPlexSans-Italic.ttf",
    "IBMPlexSans-BoldItalic.ttf",
    "Lilex-Regular.ttf",
    "Lilex-Bold.ttf",
];

pub(crate) fn app_window_title(connected_daemon_label: Option<&str>) -> String {
    format_app_window_title(APP_BUILD_BRANCH, connected_daemon_label)
}

fn format_app_window_title(branch: Option<&str>, connected_daemon_label: Option<&str>) -> String {
    let mut title = match branch.filter(|value| !value.trim().is_empty()) {
        Some(branch) => format!("{APP_NAME} [{branch}]"),
        None => APP_NAME.to_owned(),
    };

    if let Some(label) = connected_daemon_label.filter(|value| !value.trim().is_empty()) {
        title.push_str(" — ");
        title.push_str(label);
    }

    title
}

/// Load bundled fonts from disk and register them with the text system.
///
/// In a macOS `.app` bundle the fonts live under `Contents/Resources/fonts/`.
/// During development they are read from the repo-root `assets/fonts/` directory.
pub(crate) fn register_bundled_fonts(cx: &App) {
    let fonts_dir = find_fonts_dir();
    let Some(fonts_dir) = fonts_dir else {
        tracing::warn!("bundled fonts directory not found; Nerd Font icons may not render");
        return;
    };

    let mut font_data: Vec<Cow<'static, [u8]>> = Vec::new();
    for name in BUNDLED_FONT_FILES {
        let path = fonts_dir.join(name);
        match fs::read(&path) {
            Ok(bytes) => font_data.push(Cow::Owned(bytes)),
            Err(error) => tracing::warn!("failed to read font {}: {error:#}", path.display()),
        }
    }

    if font_data.is_empty() {
        return;
    }

    if let Err(error) = cx.text_system().add_fonts(font_data) {
        tracing::warn!("failed to register bundled fonts: {error:#}");
    }
}

/// Locate the `fonts/` directory, checking packaged bundle paths first, then
/// the repo-root `assets/` tree for development builds.
pub(crate) fn find_fonts_dir() -> Option<PathBuf> {
    if let Ok(exe) = env::current_exe() {
        let exe_dir = exe.parent()?;

        // macOS .app bundle: <exe>/../../Resources/fonts
        let macos_bundle = exe_dir
            .parent() // Contents/
            .map(|p| p.join("Resources").join("fonts"));
        if let Some(dir) = macos_bundle
            && dir.is_dir()
        {
            return Some(dir);
        }

        // Linux package: <exe>/../share/arbor/fonts
        let linux_share = exe_dir
            .parent() // bin/ -> package root
            .map(|p| p.join("share").join("arbor").join("fonts"));
        if let Some(dir) = linux_share
            && dir.is_dir()
        {
            return Some(dir);
        }
    }

    // Development fallback: repo-root assets/fonts relative to the crate
    let dev_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts");
    if dev_dir.is_dir() {
        return Some(dev_dir);
    }

    None
}

pub(crate) fn terminal_mono_font(cx: &App) -> gpui::Font {
    let fallbacks = FontFallbacks::from_fonts(
        TERMINAL_FONT_FAMILIES
            .iter()
            .map(|family| (*family).to_owned())
            .collect::<Vec<_>>(),
    );

    for family in TERMINAL_FONT_FAMILIES {
        let mut candidate = font(family);
        candidate.features = FontFeatures::disable_ligatures();
        candidate.fallbacks = Some(fallbacks.clone());
        let font_id = cx.text_system().resolve_font(&candidate);
        let resolved_family = cx
            .text_system()
            .get_font_for_id(font_id)
            .map(|font| font.family.to_string())
            .unwrap_or_default();
        if resolved_family == family {
            return candidate;
        }
    }

    let mut fallback = font("Menlo");
    fallback.features = FontFeatures::disable_ligatures();
    fallback.fallbacks = Some(fallbacks);
    fallback
}

#[cfg(test)]
mod tests {
    use super::format_app_window_title;

    #[test]
    fn app_window_title_omits_empty_parts() {
        assert_eq!(format_app_window_title(None, None), "Arbor");
        assert_eq!(format_app_window_title(Some(""), Some("")), "Arbor");
    }

    #[test]
    fn app_window_title_includes_compile_time_branch() {
        assert_eq!(
            format_app_window_title(Some("feature/demo"), None),
            "Arbor [feature/demo]"
        );
    }

    #[test]
    fn app_window_title_keeps_daemon_label() {
        assert_eq!(
            format_app_window_title(Some("feature/demo"), Some("local daemon")),
            "Arbor [feature/demo] — local daemon"
        );
    }
}
