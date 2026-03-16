use super::*;

// ---------------------------------------------------------------------------
// Time formatting
// ---------------------------------------------------------------------------

pub(crate) fn format_relative_time(unix_ms: u64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let age_secs = now_ms.saturating_sub(unix_ms) / 1000;

    if age_secs < 60 {
        return "just now".to_owned();
    }
    let minutes = age_secs / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

pub(crate) fn relative_time_from_rfc3339_utc(value: &str) -> Option<String> {
    parse_rfc3339_utc_millis(value).map(format_relative_time)
}

pub(crate) fn parse_rfc3339_utc_millis(value: &str) -> Option<u64> {
    let value = value.strip_suffix('Z')?;
    let (date, time) = value.split_once('T')?;

    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i32>().ok()?;
    let month = date_parts.next()?.parse::<u32>().ok()?;
    let day = date_parts.next()?.parse::<u32>().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse::<u32>().ok()?;
    let minute = time_parts.next()?.parse::<u32>().ok()?;
    let seconds_part = time_parts.next()?;
    if time_parts.next().is_some() {
        return None;
    }

    let (second, millis) = match seconds_part.split_once('.') {
        Some((second, fraction)) => {
            let second = second.parse::<u32>().ok()?;
            let digits: String = fraction
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect();
            if digits.is_empty() {
                return None;
            }
            let millis = match digits.len() {
                0 => 0,
                1 => digits.parse::<u32>().ok()? * 100,
                2 => digits.parse::<u32>().ok()? * 10,
                _ => digits[..3].parse::<u32>().ok()?,
            };
            (second, millis)
        },
        None => (seconds_part.parse::<u32>().ok()?, 0),
    };

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

    let days = days_from_civil_utc(year, month, day)?;
    if days < 0 {
        return None;
    }
    let seconds =
        days as u64 * 86_400 + u64::from(hour) * 3_600 + u64::from(minute) * 60 + u64::from(second);
    Some(
        seconds
            .saturating_mul(1_000)
            .saturating_add(u64::from(millis)),
    )
}

pub(crate) fn days_from_civil_utc(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let adjust = if month <= 2 {
        1
    } else {
        0
    };
    let shifted_year = year - adjust;
    let era = if shifted_year >= 0 {
        shifted_year / 400
    } else {
        (shifted_year - 399) / 400
    };
    let year_of_era = shifted_year - era * 400;
    let shifted_month = month as i32
        + if month > 2 {
            -3
        } else {
            9
        };
    let day_of_year = (153 * shifted_month + 2) / 5 + day as i32 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(i64::from(era) * 146_097 + i64::from(day_of_era) - 719_468)
}

pub(crate) fn format_countdown(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes:02}mn {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}mn {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

// ---------------------------------------------------------------------------
// Log formatting
// ---------------------------------------------------------------------------

pub(crate) fn format_log_entry(entry: &log_layer::LogEntry) -> String {
    let timestamp = entry
        .timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = timestamp.as_secs();
    let millis = timestamp.subsec_millis();
    let hours = (total_secs / 3600) % 24;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;
    let level_str = match entry.level {
        tracing::Level::ERROR => "ERROR",
        tracing::Level::WARN => "WARN ",
        tracing::Level::INFO => "INFO ",
        tracing::Level::DEBUG => "DEBUG",
        tracing::Level::TRACE => "TRACE",
    };
    let message = if entry.fields.is_empty() {
        entry.message.clone()
    } else {
        let fields_str: Vec<String> = entry
            .fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        format!("{} {}", entry.message, fields_str.join(" "))
    };
    format!(
        "{hours:02}:{minutes:02}:{seconds:02}.{millis:03} {level_str} {} {message}",
        entry.target
    )
}

// ---------------------------------------------------------------------------
// String truncation helpers
// ---------------------------------------------------------------------------

pub(crate) fn truncate_with_ellipsis(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_owned();
    }

    // Take max_chars - 1 characters + "…" so total stays within budget
    let truncated: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}\u{2026}")
}

pub(crate) fn truncate_middle_path_for_width(path: &Path, right_pane_width: f32) -> String {
    let path_text = path.display().to_string();
    let available_width = (right_pane_width - 110.).max(120.);
    let max_chars = ((available_width / 7.3).floor() as usize).clamp(18, 96);
    truncate_middle_text(&path_text, max_chars)
}

pub(crate) fn truncate_middle_text(input: &str, max_chars: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_chars {
        return input.to_owned();
    }

    if max_chars <= 1 {
        return "\u{2026}".to_owned();
    }

    let keep = max_chars - 1;
    let tail_keep = (keep * 3) / 5;
    let head_keep = keep.saturating_sub(tail_keep);
    let tail_start = chars.len().saturating_sub(tail_keep);

    let mut output = String::with_capacity(max_chars);
    output.extend(chars.iter().take(head_keep));
    output.push('\u{2026}');
    output.extend(chars.iter().skip(tail_start));
    output
}

// ---------------------------------------------------------------------------
// Notice / status helpers
// ---------------------------------------------------------------------------

pub(crate) fn notice_looks_like_error(notice: &str) -> bool {
    let lower = notice.to_ascii_lowercase();
    [
        "error",
        "failed",
        "invalid",
        "cannot",
        "could not",
        "missing",
        "not found",
        "denied",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub(crate) fn workspace_loading_status_label(
    diff_loading_count: usize,
    pr_loading_count: usize,
    has_resolved_pull_request_state: bool,
) -> Option<String> {
    let diff_label = match diff_loading_count {
        0 => None,
        1 => Some("1 diff".to_owned()),
        count => Some(format!("{count} diffs")),
    };
    let pr_label = match pr_loading_count {
        0 => None,
        1 => Some("1 PR".to_owned()),
        count => Some(format!("{count} PRs")),
    };
    let verb = if pr_loading_count > 0 && has_resolved_pull_request_state {
        "updating"
    } else {
        "loading"
    };

    match (pr_label, diff_label) {
        (None, None) => None,
        (Some(pr_label), None) => Some(format!("{verb} {pr_label}")),
        (None, Some(diff_label)) => Some(format!("{verb} {diff_label}")),
        (Some(pr_label), Some(diff_label)) => {
            Some(format!("{verb} {pr_label} \u{b7} {diff_label}"))
        },
    }
}

// ---------------------------------------------------------------------------
// Persisted UI state helpers
// ---------------------------------------------------------------------------

pub(crate) fn persisted_right_pane_tab(tab: RightPaneTab) -> ui_state_store::PersistedRightPaneTab {
    match tab {
        RightPaneTab::Changes => ui_state_store::PersistedRightPaneTab::Changes,
        RightPaneTab::FileTree => ui_state_store::PersistedRightPaneTab::FileTree,
        RightPaneTab::Procfile => ui_state_store::PersistedRightPaneTab::Procfile,
        RightPaneTab::Notes => ui_state_store::PersistedRightPaneTab::Notes,
    }
}

pub(crate) fn right_pane_tab_from_persisted(
    tab: Option<ui_state_store::PersistedRightPaneTab>,
) -> RightPaneTab {
    match tab.unwrap_or(ui_state_store::PersistedRightPaneTab::Changes) {
        ui_state_store::PersistedRightPaneTab::Changes => RightPaneTab::Changes,
        ui_state_store::PersistedRightPaneTab::FileTree => RightPaneTab::FileTree,
        ui_state_store::PersistedRightPaneTab::Procfile => RightPaneTab::Procfile,
        ui_state_store::PersistedRightPaneTab::Notes => RightPaneTab::Notes,
    }
}

pub(crate) fn persisted_sidebar_selection_repository_root(
    selection: Option<&ui_state_store::PersistedSidebarSelection>,
) -> Option<PathBuf> {
    match selection {
        Some(ui_state_store::PersistedSidebarSelection::Repository { root })
        | Some(ui_state_store::PersistedSidebarSelection::Worktree {
            repo_root: root, ..
        })
        | Some(ui_state_store::PersistedSidebarSelection::Outpost {
            repo_root: root, ..
        }) => Some(PathBuf::from(root)),
        None => None,
    }
}

pub(crate) fn persisted_sidebar_selection_worktree_path(
    selection: Option<&ui_state_store::PersistedSidebarSelection>,
) -> Option<PathBuf> {
    match selection {
        Some(ui_state_store::PersistedSidebarSelection::Worktree { path, .. }) => {
            Some(PathBuf::from(path))
        },
        _ => None,
    }
}

pub(crate) fn refresh_worktree_previous_local_selection(
    pending_local_selection: Option<&Path>,
    current_local_selection: Option<&Path>,
    persisted_selection: Option<&ui_state_store::PersistedSidebarSelection>,
) -> Option<PathBuf> {
    pending_local_selection
        .map(Path::to_path_buf)
        .or_else(|| current_local_selection.map(Path::to_path_buf))
        .or_else(|| persisted_sidebar_selection_worktree_path(persisted_selection))
}

pub(crate) fn persisted_sidebar_selection_outpost_index(
    selection: Option<&ui_state_store::PersistedSidebarSelection>,
    outposts: &[OutpostSummary],
) -> Option<usize> {
    let ui_state_store::PersistedSidebarSelection::Outpost { outpost_id, .. } = selection? else {
        return None;
    };

    outposts
        .iter()
        .position(|outpost| outpost.outpost_id == *outpost_id)
}

pub(crate) fn persisted_logs_tab_open(startup_ui_state: &ui_state_store::UiState) -> bool {
    startup_ui_state
        .logs_tab_open
        .unwrap_or(startup_ui_state.logs_tab_active.unwrap_or(false))
}

pub(crate) fn persisted_logs_tab_active(startup_ui_state: &ui_state_store::UiState) -> bool {
    persisted_logs_tab_open(startup_ui_state) && startup_ui_state.logs_tab_active.unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Editor / shell helpers
// ---------------------------------------------------------------------------

pub(crate) fn is_gui_editor(editor: &str) -> bool {
    let basename = Path::new(editor)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(editor);
    matches!(
        basename,
        "code"
            | "codium"
            | "subl"
            | "atom"
            | "gedit"
            | "kate"
            | "mousepad"
            | "xed"
            | "pluma"
            | "gvim"
            | "mvim"
            | "mate"
            | "bbedit"
            | "nova"
            | "zed"
            | "cursor"
            | "fleet"
            | "lite-xl"
    )
}

pub(crate) fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '/' || c == '.' || c == '-' || c == '_')
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

// ---------------------------------------------------------------------------
// Text editing helpers
// ---------------------------------------------------------------------------

pub(crate) fn char_to_byte_offset(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(byte, _)| byte)
        .unwrap_or(s.len())
}

pub(crate) fn char_count(s: &str) -> usize {
    s.chars().count()
}

pub(crate) fn apply_text_edit_action(
    text: &mut String,
    cursor: &mut usize,
    action: &TextEditAction,
) {
    *cursor = (*cursor).min(char_count(text));
    match action {
        TextEditAction::Insert(insert_text) => {
            let byte_offset = char_to_byte_offset(text, *cursor);
            text.insert_str(byte_offset, insert_text);
            *cursor += insert_text.chars().count();
        },
        TextEditAction::Backspace => {
            if *cursor == 0 {
                return;
            }
            let end = char_to_byte_offset(text, *cursor);
            let start = char_to_byte_offset(text, *cursor - 1);
            text.replace_range(start..end, "");
            *cursor -= 1;
        },
        TextEditAction::Delete => {
            let len = char_count(text);
            if *cursor >= len {
                return;
            }
            let start = char_to_byte_offset(text, *cursor);
            let end = char_to_byte_offset(text, *cursor + 1);
            text.replace_range(start..end, "");
        },
        TextEditAction::MoveLeft => {
            *cursor = (*cursor).saturating_sub(1);
        },
        TextEditAction::MoveRight => {
            *cursor = (*cursor + 1).min(char_count(text));
        },
        TextEditAction::MoveHome => {
            *cursor = 0;
        },
        TextEditAction::MoveEnd => {
            *cursor = char_count(text);
        },
    }
}

pub(crate) fn typed_text_for_keystroke(event: &KeyDownEvent) -> Option<String> {
    event
        .keystroke
        .key_char
        .as_deref()
        .or_else(|| {
            let key = event.keystroke.key.as_str();
            if key.chars().count() == 1 {
                Some(key)
            } else {
                None
            }
        })
        .map(ToOwned::to_owned)
}

pub(crate) fn text_edit_action_for_event(
    event: &KeyDownEvent,
    cx: &mut Context<ArborWindow>,
) -> Option<TextEditAction> {
    match event.keystroke.key.as_str() {
        "backspace" => return Some(TextEditAction::Backspace),
        "delete" => return Some(TextEditAction::Delete),
        "left" => return Some(TextEditAction::MoveLeft),
        "right" => return Some(TextEditAction::MoveRight),
        "home" => return Some(TextEditAction::MoveHome),
        "end" => return Some(TextEditAction::MoveEnd),
        _ => {},
    }

    if event.keystroke.modifiers.platform {
        if event.keystroke.key.as_str() == "v"
            && let Some(clipboard) = cx.read_from_clipboard()
        {
            let text = clipboard.text().unwrap_or_default();
            if !text.is_empty() {
                return Some(TextEditAction::Insert(text));
            }
        }
        return None;
    }

    if event.keystroke.modifiers.control || event.keystroke.modifiers.alt {
        return None;
    }

    typed_text_for_keystroke(event).map(TextEditAction::Insert)
}

// ---------------------------------------------------------------------------
// Syntax highlighting
// ---------------------------------------------------------------------------

pub(crate) fn highlight_lines_with_syntect(
    raw_lines: &[String],
    ext: &str,
    default_color: u32,
) -> Vec<Vec<FileViewSpan>> {
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme_set = ThemeSet::load_defaults();
    let theme = &theme_set.themes["base16-ocean.dark"];
    if let Some(syntax) = syntax_set.find_syntax_by_extension(ext) {
        let mut highlighter = HighlightLines::new(syntax, theme);
        raw_lines
            .iter()
            .map(|line| {
                // Syntect grammars loaded with load_defaults_newlines() require
                // newline-terminated lines for correct tokenisation.
                let line_nl = format!("{line}\n");
                match highlighter.highlight_line(&line_nl, &syntax_set) {
                    Ok(ranges) => ranges
                        .into_iter()
                        .filter_map(|(style, text)| {
                            let trimmed = text.trim_end_matches('\n');
                            if trimmed.is_empty() {
                                return None;
                            }
                            let c = style.foreground;
                            Some(FileViewSpan {
                                text: trimmed.to_owned(),
                                color: (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32,
                            })
                        })
                        .collect(),
                    Err(_) => vec![FileViewSpan {
                        text: line.to_owned(),
                        color: default_color,
                    }],
                }
            })
            .collect()
    } else {
        raw_lines
            .iter()
            .map(|line| {
                vec![FileViewSpan {
                    text: line.to_owned(),
                    color: default_color,
                }]
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// File icon / change helpers
// ---------------------------------------------------------------------------

pub(crate) fn file_icon_and_color(name: &str, is_dir: bool) -> (&'static str, u32) {
    if is_dir {
        return ("\u{f07b}", 0xe5c07b);
    }

    // Check full filename first
    match name {
        "Dockerfile" | ".dockerignore" => return ("\u{e7b0}", 0x61afef),
        "Makefile" | "Justfile" => return ("\u{e615}", 0x98c379),
        ".gitignore" | ".env" => return ("\u{e615}", 0x838994),
        _ => {},
    }

    // Check extension
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => ("\u{e7a8}", 0xe06c75),
        "toml" => ("\u{e615}", 0x838994),
        "py" => ("\u{e73c}", 0x61afef),
        "js" => ("\u{e74e}", 0xe5c07b),
        "ts" => ("\u{e628}", 0x61afef),
        "jsx" | "tsx" => ("\u{e7ba}", 0x56b6c2),
        "json" => ("\u{e60b}", 0xe5c07b),
        "html" => ("\u{e736}", 0xe06c75),
        "css" | "scss" | "sass" => ("\u{e749}", 0x56b6c2),
        "md" | "mdx" => ("\u{e73e}", 0x61afef),
        "yaml" | "yml" => ("\u{e615}", 0xc678dd),
        "sh" | "bash" | "zsh" => ("\u{e795}", 0x98c379),
        "go" => ("\u{e627}", 0x56b6c2),
        "c" | "h" => ("\u{e61e}", 0x61afef),
        "cpp" | "hpp" | "cc" => ("\u{e61d}", 0xe06c75),
        "java" => ("\u{e738}", 0xe06c75),
        "rb" => ("\u{e739}", 0xe06c75),
        "swift" => ("\u{e755}", 0xe06c75),
        "lock" => ("\u{f023}", 0x838994),
        "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" => ("\u{f1c5}", 0xc678dd),
        "txt" | "log" => ("\u{f15c}", 0x838994),
        "xml" => ("\u{e619}", 0xe5c07b),
        "sql" => ("\u{f1c0}", 0xe5c07b),
        _ => ("\u{f15c}", 0x838994),
    }
}

pub(crate) fn change_code(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Removed => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::TypeChange => "T",
        ChangeKind::Conflict => "U",
        ChangeKind::IntentToAdd => "I",
    }
}

// ---------------------------------------------------------------------------
// Command execution helpers
// ---------------------------------------------------------------------------

pub(crate) fn run_launch_command(
    command: &mut Command,
    operation: &str,
) -> Result<(), LaunchError> {
    let output = run_command_output(command, operation)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(LaunchError::Failed(command_failure_message(
            operation, &output,
        )))
    }
}

pub(crate) fn run_command_output(
    command: &mut Command,
    operation: &str,
) -> Result<std::process::Output, LaunchError> {
    command
        .output()
        .map_err(|error| LaunchError::Failed(format!("failed to run {operation}: {error}")))
}

pub(crate) fn command_failure_message(operation: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !stderr.is_empty() {
        return format!("{operation} failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !stdout.is_empty() {
        return format!("{operation} failed: {stdout}");
    }

    match output.status.code() {
        Some(code) => format!("{operation} failed with exit code {code}"),
        None => format!("{operation} failed"),
    }
}

// ---------------------------------------------------------------------------
// Auto-commit message helpers
// ---------------------------------------------------------------------------

pub(crate) fn auto_commit_subject(changed_files: &[ChangedFile]) -> String {
    if changed_files.len() == 1 {
        let file_label = changed_files[0]
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| changed_files[0].path.display().to_string());
        return format!("chore: update {file_label}");
    }

    let has_added = changed_files
        .iter()
        .any(|change| matches!(change.kind, ChangeKind::Added | ChangeKind::IntentToAdd));
    let has_removed = changed_files
        .iter()
        .any(|change| matches!(change.kind, ChangeKind::Removed));
    let has_renamed = changed_files
        .iter()
        .any(|change| matches!(change.kind, ChangeKind::Renamed));
    let verb = if has_added && !has_removed && !has_renamed {
        "add"
    } else if has_removed && !has_added && !has_renamed {
        "remove"
    } else if has_renamed && !has_added && !has_removed {
        "rename"
    } else {
        "update"
    };

    format!("chore: {verb} {} files", changed_files.len())
}

pub(crate) fn auto_commit_body(changed_files: &[ChangedFile]) -> String {
    let mut lines = vec!["Auto-generated by Arbor.".to_owned(), String::new()];

    for change in changed_files.iter().take(12) {
        let mut line = format!("- {} {}", change_code(change.kind), change.path.display());
        if change.additions > 0 || change.deletions > 0 {
            line.push_str(&format!(" (+{} -{})", change.additions, change.deletions));
        }
        lines.push(line);
    }

    if changed_files.len() > 12 {
        lines.push(format!("- ... and {} more", changed_files.len() - 12));
    }

    lines.join("\n")
}

pub(crate) fn default_commit_message(changed_files: &[ChangedFile]) -> String {
    format!(
        "{}\n\n{}",
        auto_commit_subject(changed_files),
        auto_commit_body(changed_files)
    )
}

pub(crate) fn auto_checkpoint_commit_message(
    changed_files: &[ChangedFile],
    agent_task: Option<&str>,
) -> String {
    let mut body_lines = vec!["Auto-checkpoint created by Arbor after an agent turn.".to_owned()];
    if let Some(task) = agent_task.map(str::trim).filter(|task| !task.is_empty()) {
        body_lines.push(format!("Task: {task}"));
    }
    body_lines.push(String::new());
    for change in changed_files.iter().take(12) {
        let mut line = format!("- {} {}", change_code(change.kind), change.path.display());
        if change.additions > 0 || change.deletions > 0 {
            line.push_str(&format!(" (+{} -{})", change.additions, change.deletions));
        }
        body_lines.push(line);
    }
    if changed_files.len() > 12 {
        body_lines.push(format!("- ... and {} more", changed_files.len() - 12));
    }

    format!("arbor: auto-checkpoint\n\n{}", body_lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Repository / branch helpers
// ---------------------------------------------------------------------------

pub(crate) fn extract_repo_name_from_url(url: &str) -> String {
    let url = url.trim();
    // Strip trailing .git
    let url = url.strip_suffix(".git").unwrap_or(url);
    // Strip trailing /
    let url = url.strip_suffix('/').unwrap_or(url);
    // Get the last path component
    if let Some(pos) = url.rfind('/') {
        url[pos + 1..].to_owned()
    } else if let Some(pos) = url.rfind(':') {
        // SSH-style: git@github.com:user/repo
        let after_colon = &url[pos + 1..];
        if let Some(slash_pos) = after_colon.rfind('/') {
            after_colon[slash_pos + 1..].to_owned()
        } else {
            after_colon.to_owned()
        }
    } else {
        String::new()
    }
}

pub(crate) fn repository_display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn branch_divergence_summary(worktree_path: &Path) -> Option<BranchDivergenceSummary> {
    let repo = git2::Repository::open(worktree_path).ok()?;
    let head = repo.head().ok()?;
    if !head.is_branch() {
        return None;
    }

    let branch_name = head.shorthand()?;
    let branch = repo
        .find_branch(branch_name, git2::BranchType::Local)
        .ok()?;
    let upstream = branch.upstream().ok()?;
    let head_oid = branch.get().target()?;
    let upstream_oid = upstream.get().target()?;
    let (ahead, behind) = repo.graph_ahead_behind(head_oid, upstream_oid).ok()?;

    Some(BranchDivergenceSummary { ahead, behind })
}

pub(crate) fn should_seed_repo_root_from_cwd(
    store_file_exists: bool,
    loaded_roots_were_empty: bool,
) -> bool {
    // Seed from CWD on first run (no store file), or when there are existing
    // saved roots and CWD is simply not listed yet. If the store exists and is
    // explicitly empty, preserve that empty state across restarts.
    !store_file_exists || !loaded_roots_were_empty
}

pub(crate) fn short_branch(value: &str) -> String {
    worktree::short_branch(value)
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

pub(crate) fn expand_home_path(path: &str) -> Result<PathBuf, PathError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(PathError::EmptyPath);
    }

    if trimmed == "~" {
        return user_home_dir();
    }

    if let Some(suffix) = trimmed.strip_prefix("~/") {
        return user_home_dir().map(|home| home.join(suffix));
    }

    Ok(PathBuf::from(trimmed))
}

pub(crate) fn user_home_dir() -> Result<PathBuf, PathError> {
    env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| PathError::NoHomeDir)
}

// ---------------------------------------------------------------------------
// Worktree naming helpers
// ---------------------------------------------------------------------------

pub(crate) fn sanitize_worktree_name(value: &str) -> String {
    let mut sanitized = String::new();
    let mut previous_dash = false;

    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            sanitized.push(character.to_ascii_lowercase());
            previous_dash = false;
            continue;
        }

        if character == '-' || character == '_' || character == '.' {
            sanitized.push(character);
            previous_dash = false;
            continue;
        }

        if !previous_dash && !sanitized.is_empty() {
            sanitized.push('-');
            previous_dash = true;
        }
    }

    while sanitized.ends_with('-') {
        let _ = sanitized.pop();
    }

    sanitized
}

pub(crate) fn derive_branch_name(worktree_name: &str) -> String {
    let sanitized = sanitize_worktree_name(worktree_name);
    if sanitized.is_empty() {
        "worktree".to_owned()
    } else {
        sanitized
    }
}

pub(crate) fn derive_branch_name_for_repo_with_login(
    repo_root: &Path,
    worktree_name: &str,
    github_login: Option<&str>,
) -> String {
    if repo_root.as_os_str().is_empty() || !repo_root.exists() {
        return derive_branch_name(worktree_name);
    }
    let repo_root = worktree::repo_root(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    derive_branch_name_with_repo_config(&repo_root, worktree_name, github_login)
}

pub(crate) fn derive_branch_name_with_repo_config(
    repo_root: &Path,
    worktree_name: &str,
    github_login: Option<&str>,
) -> String {
    let base_name = derive_branch_name(worktree_name);
    let Some(config) = repo_config::load_repo_config(repo_root) else {
        return base_name;
    };

    let prefix = match config.branch.prefix_mode {
        Some(repo_config::RepoBranchPrefixMode::None) | None => None,
        Some(repo_config::RepoBranchPrefixMode::GitAuthor) => {
            git_branch_prefix_from_author(repo_root)
        },
        Some(repo_config::RepoBranchPrefixMode::GithubUser) => github_login
            .map(sanitize_worktree_name)
            .filter(|value| !value.is_empty()),
        Some(repo_config::RepoBranchPrefixMode::Custom) => config
            .branch
            .prefix
            .as_deref()
            .map(sanitize_worktree_name)
            .filter(|value| !value.is_empty()),
    };

    match prefix {
        Some(prefix) => format!("{prefix}/{base_name}"),
        None => base_name,
    }
}

pub(crate) fn git_branch_prefix_from_author(repo_root: &Path) -> Option<String> {
    let mut command = create_command("git");
    command
        .arg("-C")
        .arg(repo_root)
        .args(["config", "--get", "user.name"]);
    let output = run_command_output(&mut command, "read git author").ok()?;
    if !output.status.success() {
        return None;
    }

    let author = String::from_utf8_lossy(&output.stdout);
    let sanitized = sanitize_worktree_name(author.trim());
    (!sanitized.is_empty()).then_some(sanitized)
}

pub(crate) fn build_managed_worktree_path(
    repo_name: &str,
    worktree_name: &str,
) -> Result<PathBuf, PathError> {
    let home_dir = user_home_dir()?;
    Ok(home_dir
        .join(".arbor")
        .join("worktrees")
        .join(repo_name)
        .join(worktree_name))
}

// ---------------------------------------------------------------------------
// Worktree notes helpers
// ---------------------------------------------------------------------------

pub(crate) fn worktree_notes_load_is_current(
    started_generation: u64,
    current_generation: u64,
    current_path: Option<&Path>,
    expected_path: &Path,
    started_edit_generation: u64,
    current_edit_generation: u64,
) -> bool {
    started_generation == current_generation
        && current_path == Some(expected_path)
        && started_edit_generation == current_edit_generation
}

pub(crate) fn worktree_notes_storage_path(worktree_path: &Path) -> PathBuf {
    worktree_path.join(".arbor").join("notes.md")
}

// ---------------------------------------------------------------------------
// Task template helpers
// ---------------------------------------------------------------------------

pub(crate) fn load_task_templates_for_repo(repo_root: &Path) -> Vec<TaskTemplate> {
    let tasks_dir = repo_task_templates_dir(repo_root);
    let Ok(entries) = fs::read_dir(&tasks_dir) else {
        return Vec::new();
    };

    let mut tasks = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        if let Some(task) = parse_task_template(&path, repo_root) {
            tasks.push(task);
        }
    }
    tasks.sort_by(|left, right| left.name.cmp(&right.name));
    tasks
}

pub(crate) fn repo_task_templates_dir(repo_root: &Path) -> PathBuf {
    let relative_dir = repo_config::load_repo_config(repo_root)
        .and_then(|config| config.tasks.directory)
        .unwrap_or_else(|| ".arbor/tasks".to_owned());
    repo_root.join(relative_dir)
}

pub(crate) fn parse_task_template(path: &Path, repo_root: &Path) -> Option<TaskTemplate> {
    let content = fs::read_to_string(path).ok()?;
    parse_task_template_content(path, repo_root, &content)
}

pub(crate) fn parse_task_template_content(
    path: &Path,
    repo_root: &Path,
    content: &str,
) -> Option<TaskTemplate> {
    let mut name = path.file_stem()?.to_string_lossy().into_owned();
    let mut description = None;
    let mut agent = None;
    let mut body = content;

    if content
        .lines()
        .next()
        .is_some_and(|line| line.trim() == "---")
    {
        let mut frontmatter = Vec::new();
        let mut body_start_offset = None;
        let mut offset = 0usize;
        for (index, line) in content.lines().enumerate() {
            offset += line.len() + 1;
            if index == 0 {
                continue;
            }
            if line.trim() == "---" {
                body_start_offset = Some(offset);
                break;
            }
            frontmatter.push(line);
        }

        if let Some(start) = body_start_offset {
            body = &content[start..];
            for line in frontmatter {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let key = key.trim().to_ascii_lowercase();
                let value = value.trim().trim_matches('"').trim_matches('\'');
                match key.as_str() {
                    "name" if !value.is_empty() => name = value.to_owned(),
                    "title" if !value.is_empty() => name = value.to_owned(),
                    "description" if !value.is_empty() => description = Some(value.to_owned()),
                    "agent" => agent = AgentPresetKind::from_key(value),
                    _ => {},
                }
            }
        }
    }

    let mut prompt_lines = Vec::new();
    let mut found_prompt_line = false;
    let mut heading_name = None;

    for line in body.lines() {
        let trimmed = line.trim();
        if !found_prompt_line {
            if trimmed.is_empty() {
                continue;
            }

            if heading_name.is_none()
                && let Some(heading) = trimmed.strip_prefix("# ")
            {
                let heading = heading.trim();
                if !heading.is_empty() {
                    heading_name = Some(heading.to_owned());
                }
                continue;
            }

            if let Some((raw_key, raw_value)) = trimmed.split_once(':') {
                let key = raw_key.trim().to_ascii_lowercase();
                let value = raw_value.trim().trim_matches('"').trim_matches('\'');
                match key.as_str() {
                    "agent" => {
                        if agent.is_none() {
                            agent = AgentPresetKind::from_key(value);
                        }
                        continue;
                    },
                    "description" => {
                        if description.is_none() && !value.is_empty() {
                            description = Some(value.to_owned());
                        }
                        continue;
                    },
                    _ => {},
                }
            }
        }

        found_prompt_line = true;
        prompt_lines.push(line);
    }

    if let Some(heading_name) = heading_name
        && name == path.file_stem()?.to_string_lossy()
    {
        name = heading_name;
    }

    let prompt = prompt_lines.join("\n").trim().to_owned();
    if prompt.is_empty() {
        return None;
    }
    let description = description.unwrap_or_else(|| {
        prompt
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
            .unwrap_or("Task template")
            .to_owned()
    });

    Some(TaskTemplate {
        name,
        description,
        prompt,
        agent,
        path: path.to_path_buf(),
        repo_root: repo_root.to_path_buf(),
    })
}

pub(crate) fn shell_quote(value: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        let escaped = value.replace('"', "\"\"");
        format!("\"{escaped}\"")
    }

    #[cfg(not(target_os = "windows"))]
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

// ---------------------------------------------------------------------------
// Collapsed repository indices
// ---------------------------------------------------------------------------

pub(crate) fn collapsed_repository_indices_from_group_keys(
    repositories: &[RepositorySummary],
    collapsed_group_keys: &[String],
) -> HashSet<usize> {
    let collapsed_group_keys: HashSet<&str> =
        collapsed_group_keys.iter().map(String::as_str).collect();
    repositories
        .iter()
        .enumerate()
        .filter_map(|(index, repository)| {
            collapsed_group_keys
                .contains(repository.group_key.as_str())
                .then_some(index)
        })
        .collect()
}

/// Return the current process's resident set size in bytes.
#[cfg(unix)]
pub(crate) fn self_rss_bytes() -> Option<u64> {
    let pid = std::process::id();
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let kib: u64 = text.trim().parse().ok()?;
    Some(kib.saturating_mul(1024))
}

#[cfg(not(unix))]
pub(crate) fn self_rss_bytes() -> Option<u64> {
    None
}

pub(crate) fn format_memory_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else {
        format!("{} MB", bytes / 1_048_576)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {
        crate::{
            AgentPresetKind, OutpostSummary, RepositorySummary, auto_commit_body,
            auto_commit_subject, checkout::CheckoutKind, repository_store, ui_state_store,
        },
        arbor_core::changes::{ChangeKind, ChangedFile},
        std::{
            collections::HashSet,
            env, fs,
            path::{Path, PathBuf},
            time::SystemTime,
        },
    };

    fn create_temp_test_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_else(|error| panic!("system clock before unix epoch: {error}"))
            .as_nanos();
        let path = env::temp_dir().join(format!("arbor-gui-helpers-{prefix}-{unique}"));
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("failed to create temp dir `{}`: {error}", path.display())
        });
        path
    }

    #[test]
    fn sanitizes_worktree_name_for_branch_and_path() {
        let sanitized = crate::sanitize_worktree_name("  Remote SSH / Demo  ");
        assert_eq!(sanitized, "remote-ssh-demo");
    }

    #[test]
    fn derives_default_branch_name_when_empty() {
        let branch = crate::derive_branch_name(" !!! ");
        assert_eq!(branch, "worktree");
    }

    #[test]
    fn derive_branch_name_uses_custom_repo_prefix_mode() {
        let dir = create_temp_test_dir("branch-prefix");
        fs::write(
            dir.join("arbor.toml"),
            "[branch]\nprefix_mode = \"custom\"\nprefix = \"team\"\n",
        )
        .unwrap_or_else(|error| panic!("failed to write repo config: {error}"));

        assert_eq!(
            crate::derive_branch_name_with_repo_config(&dir, "Auth Fix", None),
            "team/auth-fix"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repo_task_templates_dir_honors_repo_config_override() {
        let dir = create_temp_test_dir("task-dir");
        fs::write(dir.join("arbor.toml"), "[tasks]\ndirectory = \"prompts\"\n")
            .unwrap_or_else(|error| panic!("failed to write repo config: {error}"));

        assert_eq!(crate::repo_task_templates_dir(&dir), dir.join("prompts"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn persisted_sidebar_selection_helpers_restore_saved_targets() {
        let worktree_selection = ui_state_store::PersistedSidebarSelection::Worktree {
            repo_root: "/tmp/repo".to_owned(),
            path: "/tmp/repo/issue-42".to_owned(),
        };
        assert_eq!(
            crate::persisted_sidebar_selection_repository_root(Some(&worktree_selection)),
            Some(PathBuf::from("/tmp/repo"))
        );
        assert_eq!(
            crate::persisted_sidebar_selection_worktree_path(Some(&worktree_selection)),
            Some(PathBuf::from("/tmp/repo/issue-42"))
        );

        let outpost_selection = ui_state_store::PersistedSidebarSelection::Outpost {
            repo_root: "/tmp/repo".to_owned(),
            outpost_id: "outpost-1".to_owned(),
        };
        let outposts = vec![OutpostSummary {
            outpost_id: "outpost-1".to_owned(),
            repo_root: PathBuf::from("/tmp/repo"),
            remote_path: "/srv/repo".to_owned(),
            label: "prod".to_owned(),
            branch: "main".to_owned(),
            host_name: "prod".to_owned(),
            hostname: "prod.example.com".to_owned(),
            status: arbor_core::outpost::OutpostStatus::Available,
        }];
        assert_eq!(
            crate::persisted_sidebar_selection_outpost_index(Some(&outpost_selection), &outposts),
            Some(0)
        );
    }

    #[test]
    fn collapsed_repository_indices_match_saved_group_keys() {
        let repo_a = RepositorySummary::from_checkout_roots(
            PathBuf::from("/tmp/repo-a"),
            "repo-a".to_owned(),
            vec![repository_store::RepositoryCheckoutRoot {
                path: PathBuf::from("/tmp/repo-a"),
                kind: CheckoutKind::LinkedWorktree,
            }],
        );
        let repo_b = RepositorySummary::from_checkout_roots(
            PathBuf::from("/tmp/repo-b"),
            "repo-b".to_owned(),
            vec![repository_store::RepositoryCheckoutRoot {
                path: PathBuf::from("/tmp/repo-b"),
                kind: CheckoutKind::LinkedWorktree,
            }],
        );

        assert_eq!(
            crate::collapsed_repository_indices_from_group_keys(&[repo_a, repo_b], &[
                "repo-b".to_owned(),
                "missing".to_owned()
            ],),
            HashSet::from([1])
        );
    }

    #[test]
    fn refresh_worktree_previous_local_selection_prefers_pending_created_path() {
        let persisted = ui_state_store::PersistedSidebarSelection::Worktree {
            repo_root: "/tmp/repo".to_owned(),
            path: "/tmp/repo/old".to_owned(),
        };

        assert_eq!(
            crate::refresh_worktree_previous_local_selection(
                Some(Path::new("/tmp/repo/new")),
                Some(Path::new("/tmp/repo/current")),
                Some(&persisted),
            ),
            Some(PathBuf::from("/tmp/repo/new"))
        );
    }

    #[test]
    fn persisted_logs_tab_state_only_restores_active_when_open() {
        let state = ui_state_store::UiState {
            logs_tab_open: Some(false),
            logs_tab_active: Some(true),
            ..ui_state_store::UiState::default()
        };
        assert!(!crate::persisted_logs_tab_open(&state));
        assert!(!crate::persisted_logs_tab_active(&state));

        let state = ui_state_store::UiState {
            logs_tab_open: Some(true),
            logs_tab_active: Some(true),
            ..ui_state_store::UiState::default()
        };
        assert!(crate::persisted_logs_tab_open(&state));
        assert!(crate::persisted_logs_tab_active(&state));
    }

    #[test]
    fn truncate_middle_text_keeps_tail_visible() {
        let truncated = crate::truncate_middle_text("src/some/really/long/path/main.rs", 16);
        assert!(truncated.contains('\u{2026}'));
        assert!(truncated.ends_with("main.rs"));
    }

    #[test]
    fn truncate_middle_text_returns_original_when_short() {
        let input = "src/main.rs";
        let truncated = crate::truncate_middle_text(input, 32);
        assert_eq!(truncated, input);
    }

    #[test]
    fn auto_commit_subject_uses_filename_for_single_change() {
        let changed_files = vec![ChangedFile {
            path: PathBuf::from("src/main.rs"),
            kind: ChangeKind::Modified,
            additions: 4,
            deletions: 1,
        }];

        let subject = auto_commit_subject(&changed_files);
        assert_eq!(subject, "chore: update main.rs");
    }

    #[test]
    fn auto_commit_body_includes_stats_and_overflow_line() {
        let changed_files = (0..13)
            .map(|index| ChangedFile {
                path: PathBuf::from(format!("src/file-{index}.rs")),
                kind: ChangeKind::Modified,
                additions: index + 1,
                deletions: index,
            })
            .collect::<Vec<_>>();

        let body = auto_commit_body(&changed_files);
        assert!(body.contains("Auto-generated by Arbor."));
        assert!(body.contains("- M src/file-0.rs (+1 -0)"));
        assert!(body.contains("- ... and 1 more"));
    }

    #[test]
    fn format_countdown_uses_minute_suffix_requested_by_ui() {
        assert_eq!(
            crate::format_countdown(std::time::Duration::from_secs(3 * 60)),
            "3mn 00s"
        );
        assert_eq!(
            crate::format_countdown(std::time::Duration::from_secs(95)),
            "1mn 35s"
        );
    }

    #[test]
    fn format_countdown_keeps_hour_component() {
        assert_eq!(
            crate::format_countdown(std::time::Duration::from_secs(3723)),
            "1h 02mn 03s"
        );
    }

    #[test]
    fn seed_repo_root_from_cwd_when_store_file_missing() {
        assert!(crate::should_seed_repo_root_from_cwd(false, false));
        assert!(crate::should_seed_repo_root_from_cwd(false, true));
    }

    #[test]
    fn does_not_seed_repo_root_from_cwd_when_store_is_explicitly_empty() {
        assert!(!crate::should_seed_repo_root_from_cwd(true, true));
    }

    #[test]
    fn seed_repo_root_from_cwd_when_store_has_saved_roots() {
        assert!(crate::should_seed_repo_root_from_cwd(true, false));
    }

    #[test]
    fn truncate_with_ellipsis_short_string_unchanged() {
        let result = crate::truncate_with_ellipsis("hello", 11);
        assert_eq!(result, "hello");
    }

    #[test]
    fn truncate_with_ellipsis_exact_limit_unchanged() {
        let result = crate::truncate_with_ellipsis("12345678901", 11);
        assert_eq!(result, "12345678901");
    }

    #[test]
    fn truncate_with_ellipsis_over_limit_adds_ellipsis() {
        let result = crate::truncate_with_ellipsis("123456789012", 11);
        assert_eq!(result, "1234567890\u{2026}");
        assert_eq!(result.chars().count(), 11);
    }

    #[test]
    fn truncate_with_ellipsis_tab_label_cases() {
        // These are the actual tab titles that need to show "…"
        let cases = [
            "nvim: CHANGELOG.md",
            "nvim: CLAUDE.md",
            "nvim: Cargo.lock",
            "nvim: Cargo.toml",
            "nvim: clippy.toml",
            "nvim: LICENSE",
            "nvim: AGENTS.md",
        ];
        for title in cases {
            let result = crate::truncate_with_ellipsis(title, 11);
            assert!(
                result.chars().count() <= 11,
                "'{result}' from '{title}' is {} chars, exceeds 11",
                result.chars().count()
            );
            if title.chars().count() > 11 {
                assert!(
                    result.ends_with('\u{2026}'),
                    "'{result}' from '{title}' should end with ellipsis"
                );
            }
        }
    }

    #[test]
    fn parse_rfc3339_utc_millis_parses_issue_timestamps() {
        assert_eq!(
            crate::parse_rfc3339_utc_millis("2026-03-14T20:31:45Z"),
            Some(1_773_520_305_000)
        );
        assert_eq!(
            crate::parse_rfc3339_utc_millis("2026-03-14T20:31:45.987Z"),
            Some(1_773_520_305_987)
        );
        assert_eq!(crate::parse_rfc3339_utc_millis("not-a-timestamp"), None);
    }

    #[test]
    fn worktree_notes_load_is_current_rejects_newer_live_edits() {
        let path = Path::new("/tmp/repo/.arbor/notes.md");

        assert!(crate::worktree_notes_load_is_current(
            4,
            4,
            Some(path),
            path,
            10,
            10,
        ));
        assert!(!crate::worktree_notes_load_is_current(
            4,
            4,
            Some(path),
            path,
            11,
            10,
        ));
    }

    #[test]
    fn parse_task_template_supports_frontmatter_description_and_agent() {
        let repo_root = Path::new("/tmp/repo");
        let path = repo_root.join(".arbor/tasks/review.md");
        let content = r#"---
name: Review PR
description: Review the riskiest changes first
agent: codex
---
Review the current branch and summarize the highest-risk changes.
"#;

        let task = crate::parse_task_template_content(&path, repo_root, content)
            .unwrap_or_else(|| panic!("task template should parse"));
        assert_eq!(task.name, "Review PR");
        assert_eq!(task.description, "Review the riskiest changes first");
        assert_eq!(task.agent, Some(AgentPresetKind::Codex));
        assert_eq!(
            task.prompt,
            "Review the current branch and summarize the highest-risk changes."
        );
    }

    #[test]
    fn parse_task_template_supports_heading_and_agent_metadata() {
        let repo_root = Path::new("/tmp/repo");
        let path = repo_root.join(".arbor/tasks/review.md");
        let content = r#"# Review PR

Agent: Codex
Description: Review the current branch before merge

Review the current branch and summarize the highest-risk changes.
"#;

        let task = crate::parse_task_template_content(&path, repo_root, content)
            .unwrap_or_else(|| panic!("task template should parse"));
        assert_eq!(task.name, "Review PR");
        assert_eq!(task.description, "Review the current branch before merge");
        assert_eq!(task.agent, Some(AgentPresetKind::Codex));
        assert_eq!(
            task.prompt,
            "Review the current branch and summarize the highest-risk changes."
        );
    }
}
