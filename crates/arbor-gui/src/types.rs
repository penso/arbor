use {
    crate::{
        checkout::CheckoutKind,
        github_service,
        terminal_backend::{TerminalCursor, TerminalModes, TerminalStyledLine},
        terminal_daemon_http,
        terminal_runtime::SharedTerminalRuntime,
    },
    arbor_core::SessionId,
    gpui::{Context, Pixels, Window, prelude::*},
    serde::{Deserialize, Serialize},
    std::{
        collections::HashMap,
        path::PathBuf,
        process::{Child, Stdio},
        sync::Arc,
        time::Instant,
    },
};

#[derive(Debug, Clone)]
pub(crate) struct WorktreeSummary {
    pub(crate) group_key: String,
    pub(crate) checkout_kind: CheckoutKind,
    pub(crate) repo_root: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) label: String,
    pub(crate) branch: String,
    pub(crate) is_primary_checkout: bool,
    pub(crate) pr_number: Option<u64>,
    pub(crate) pr_url: Option<String>,
    pub(crate) pr_details: Option<github_service::PrDetails>,
    pub(crate) diff_summary: Option<arbor_core::changes::DiffLineSummary>,
    pub(crate) agent_state: Option<arbor_core::agent::AgentState>,
    pub(crate) agent_task: Option<String>,
    pub(crate) last_activity_unix_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct RepositorySummary {
    pub(crate) group_key: String,
    pub(crate) root: PathBuf,
    pub(crate) checkout_roots: Vec<crate::repository_store::RepositoryCheckoutRoot>,
    pub(crate) label: String,
    pub(crate) avatar_url: Option<String>,
    pub(crate) github_repo_slug: Option<String>,
}

#[derive(Clone)]
pub(crate) struct TerminalSession {
    pub(crate) id: u64,
    pub(crate) daemon_session_id: SessionId,
    pub(crate) worktree_path: PathBuf,
    pub(crate) title: String,
    pub(crate) last_command: Option<String>,
    pub(crate) pending_command: String,
    pub(crate) command: String,
    pub(crate) state: TerminalState,
    pub(crate) exit_code: Option<i32>,
    pub(crate) updated_at_unix_ms: Option<u64>,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) generation: u64,
    pub(crate) output: String,
    pub(crate) styled_output: Vec<TerminalStyledLine>,
    pub(crate) cursor: Option<TerminalCursor>,
    pub(crate) modes: TerminalModes,
    pub(crate) last_runtime_sync_at: Option<Instant>,
    pub(crate) runtime: Option<SharedTerminalRuntime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalState {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CenterTab {
    Terminal(u64),
    Diff(u64),
    FileView(u64),
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RightPaneTab {
    Changes,
    FileTree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AgentPresetKind {
    Codex,
    Claude,
    Pi,
    OpenCode,
    Copilot,
}

impl AgentPresetKind {
    pub(crate) const ORDER: [Self; 5] = [
        Self::Codex,
        Self::Claude,
        Self::Pi,
        Self::OpenCode,
        Self::Copilot,
    ];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
            Self::Copilot => "copilot",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Pi => "Pi",
            Self::OpenCode => "OpenCode",
            Self::Copilot => "Copilot",
        }
    }

    pub(crate) fn fallback_icon(self) -> &'static str {
        match self {
            Self::Codex => "\u{f121}",
            Self::Claude => "C",
            Self::Pi => "P",
            Self::OpenCode => "\u{f085}",
            Self::Copilot => "\u{f09b}",
        }
    }

    pub(crate) fn default_command(self) -> &'static str {
        match self {
            Self::Codex => {
                "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox -c model_reasoning_summary=\"detailed\" -c model_supports_reasoning_summaries=true"
            },
            Self::Claude => "claude --dangerously-skip-permissions",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
            Self::Copilot => "copilot --allow-all",
        }
    }

    pub(crate) fn executable_name(self) -> &'static str {
        self.key()
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            "pi" => Some(Self::Pi),
            "opencode" => Some(Self::OpenCode),
            "copilot" => Some(Self::Copilot),
            _ => None,
        }
    }

    pub(crate) fn cycle(self, reverse: bool) -> Self {
        let current = Self::ORDER
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        if reverse {
            Self::ORDER[(current + Self::ORDER.len() - 1) % Self::ORDER.len()]
        } else {
            Self::ORDER[(current + 1) % Self::ORDER.len()]
        }
    }

    /// Check if the default command for this preset is available in PATH.
    pub(crate) fn is_installed(self) -> bool {
        crate::is_command_in_path(self.executable_name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentPreset {
    pub(crate) kind: AgentPresetKind,
    pub(crate) command: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SettingsModal {
    pub(crate) active_control: SettingsControl,
    pub(crate) daemon_bind_mode: DaemonBindMode,
    pub(crate) initial_daemon_bind_mode: DaemonBindMode,
    pub(crate) notifications: bool,
    pub(crate) daemon_auth_token: String,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsControl {
    DaemonBindMode,
    Notifications,
}

impl SettingsControl {
    pub(crate) fn cycle(self, reverse: bool) -> Self {
        const ORDER: [SettingsControl; 2] = [
            SettingsControl::DaemonBindMode,
            SettingsControl::Notifications,
        ];
        let current = ORDER
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        if reverse {
            ORDER[(current + ORDER.len() - 1) % ORDER.len()]
        } else {
            ORDER[(current + 1) % ORDER.len()]
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DaemonBindMode {
    Localhost,
    AllInterfaces,
}

impl DaemonBindMode {
    pub(crate) fn from_config(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("localhost" | "local" | "loopback" | "127.0.0.1") => Self::Localhost,
            Some("all" | "all-interfaces" | "public" | "0.0.0.0") => Self::AllInterfaces,
            _ => Self::AllInterfaces,
        }
    }

    pub(crate) fn as_config_value(self) -> &'static str {
        match self {
            Self::Localhost => "localhost",
            Self::AllInterfaces => "all-interfaces",
        }
    }
}

pub(crate) enum SettingsModalInputEvent {
    CycleControl(bool),
    SelectDaemonBindMode(DaemonBindMode),
    ToggleActiveControl,
    ToggleNotifications,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagePresetsModal {
    pub(crate) active_preset: AgentPresetKind,
    pub(crate) command: String,
    pub(crate) command_cursor: usize,
    pub(crate) error: Option<String>,
}

pub(crate) enum PresetsModalInputEvent {
    SetActivePreset(AgentPresetKind),
    CycleActivePreset(bool),
    Edit(TextEditAction),
    RestoreDefault,
    ClearError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoPreset {
    pub(crate) name: String,
    pub(crate) icon: String,
    pub(crate) command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepoPresetModalField {
    Icon,
    Name,
    Command,
}

impl RepoPresetModalField {
    pub(crate) const ORDER: [Self; 3] = [Self::Icon, Self::Name, Self::Command];

    pub(crate) fn next(self) -> Self {
        let index = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(index + 1) % Self::ORDER.len()]
    }

    pub(crate) fn prev(self) -> Self {
        let index = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(index + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ManageRepoPresetsModal {
    pub(crate) editing_index: Option<usize>,
    pub(crate) icon: String,
    pub(crate) icon_cursor: usize,
    pub(crate) name: String,
    pub(crate) name_cursor: usize,
    pub(crate) command: String,
    pub(crate) command_cursor: usize,
    pub(crate) active_tab: RepoPresetModalTab,
    pub(crate) active_field: RepoPresetModalField,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepoPresetModalTab {
    Edit,
    LocalPreset,
}

pub(crate) enum RepoPresetsModalInputEvent {
    SetActiveTab(RepoPresetModalTab),
    SetActiveField(RepoPresetModalField),
    MoveActiveField(bool),
    Edit(TextEditAction),
    ClearError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitActionKind {
    Commit,
    Push,
    CreatePullRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreeQuickAction {
    OpenFinder,
    CopyPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuickActionSubmenu {
    Ide,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExternalLauncherKind {
    Command(&'static str),
    MacApp(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExternalLauncher {
    pub(crate) label: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) icon_color: u32,
    pub(crate) kind: ExternalLauncherKind,
}

#[derive(Debug, Clone)]
pub(crate) struct FileTreeEntry {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) is_dir: bool,
    pub(crate) depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffLineKind {
    FileHeader,
    Context,
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone)]
pub(crate) struct DiffLine {
    pub(crate) left_line_number: Option<usize>,
    pub(crate) right_line_number: Option<usize>,
    pub(crate) left_text: String,
    pub(crate) right_text: String,
    pub(crate) kind: DiffLineKind,
}

#[derive(Debug, Clone)]
pub(crate) struct DiffSession {
    pub(crate) id: u64,
    pub(crate) worktree_path: PathBuf,
    pub(crate) title: String,
    pub(crate) raw_lines: Arc<[DiffLine]>,
    pub(crate) raw_file_row_indices: HashMap<PathBuf, usize>,
    pub(crate) lines: Arc<[DiffLine]>,
    pub(crate) file_row_indices: HashMap<PathBuf, usize>,
    pub(crate) wrapped_columns: usize,
    pub(crate) is_loading: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct FileViewSpan {
    pub(crate) text: String,
    pub(crate) color: u32,
}

#[derive(Debug, Clone)]
pub(crate) enum FileViewContent {
    Text {
        highlighted: Arc<[Vec<FileViewSpan>]>,
        raw_lines: Vec<String>,
        dirty: bool,
    },
    Image(PathBuf),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FileViewCursor {
    pub(crate) line: usize,
    pub(crate) col: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct FileViewSession {
    pub(crate) id: u64,
    pub(crate) worktree_path: PathBuf,
    pub(crate) file_path: PathBuf,
    pub(crate) title: String,
    pub(crate) content: FileViewContent,
    pub(crate) is_loading: bool,
    pub(crate) cursor: FileViewCursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DraggedPaneDivider {
    Left,
    Right,
}

impl Render for DraggedPaneDivider {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Identifies a sidebar item — either a local worktree or a remote outpost.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum SidebarItemId {
    Worktree(PathBuf),
    Outpost(String),
}

/// Payload carried during a drag operation on a sidebar item.
#[derive(Debug, Clone)]
pub(crate) struct DraggedSidebarItem {
    pub(crate) item_id: SidebarItemId,
    pub(crate) group_key: String,
    pub(crate) label: String,
    pub(crate) icon: String,
    pub(crate) icon_color: u32,
    pub(crate) bg_color: u32,
    pub(crate) border_color: u32,
    pub(crate) text_color: u32,
}

impl Render for DraggedSidebarItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use gpui::{FontWeight, div, prelude::*, px, rgb};

        div()
            .w(px(220.))
            .font_family(crate::FONT_MONO)
            .rounded_sm()
            .border_1()
            .border_color(rgb(self.border_color))
            .bg(rgb(self.bg_color))
            .px_2()
            .py_1()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .opacity(0.9)
            .child(
                div()
                    .flex_none()
                    .w(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(16.))
                    .text_color(rgb(self.icon_color))
                    .child(self.icon.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(self.text_color))
                    .child(self.label.clone()),
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalGridPosition {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalSelection {
    pub(crate) session_id: u64,
    pub(crate) anchor: TerminalGridPosition,
    pub(crate) head: TerminalGridPosition,
}

#[derive(Debug, Clone)]
pub(crate) struct OutpostSummary {
    pub(crate) outpost_id: String,
    pub(crate) repo_root: PathBuf,
    pub(crate) remote_path: String,
    pub(crate) label: String,
    pub(crate) branch: String,
    pub(crate) host_name: String,
    pub(crate) hostname: String,
    pub(crate) status: arbor_core::outpost::OutpostStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateModalTab {
    LocalWorktree,
    RemoteOutpost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateOutpostField {
    HostSelector,
    CloneUrl,
    OutpostName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateWorktreeField {
    RepositoryPath,
    WorktreeName,
}

#[derive(Debug, Clone)]
pub(crate) struct CreateModal {
    pub(crate) tab: CreateModalTab,
    // Worktree fields
    pub(crate) repository_path: String,
    pub(crate) repository_path_cursor: usize,
    pub(crate) worktree_name: String,
    pub(crate) worktree_name_cursor: usize,
    pub(crate) checkout_kind: CheckoutKind,
    pub(crate) worktree_active_field: CreateWorktreeField,
    // Outpost fields
    pub(crate) host_index: usize,
    pub(crate) host_dropdown_open: bool,
    pub(crate) clone_url: String,
    pub(crate) clone_url_cursor: usize,
    pub(crate) outpost_name: String,
    pub(crate) outpost_name_cursor: usize,
    pub(crate) outpost_active_field: CreateOutpostField,
    // Shared
    pub(crate) is_creating: bool,
    pub(crate) creating_status: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct GitHubAuthModal {
    pub(crate) user_code: String,
    pub(crate) verification_url: String,
}

pub(crate) enum ModalInputEvent {
    SetActiveField(CreateWorktreeField),
    MoveActiveField,
    Edit(TextEditAction),
    ClearError,
}

pub(crate) enum OutpostModalInputEvent {
    SetActiveField(CreateOutpostField),
    MoveActiveField(bool),
    CycleHost(bool),
    SelectHost(usize),
    ToggleHostDropdown,
    Edit(TextEditAction),
    ClearError,
}

#[derive(Clone)]
pub(crate) struct ManageHostsModal {
    pub(crate) adding: bool,
    pub(crate) name: String,
    pub(crate) name_cursor: usize,
    pub(crate) hostname: String,
    pub(crate) hostname_cursor: usize,
    pub(crate) user: String,
    pub(crate) user_cursor: usize,
    pub(crate) active_field: ManageHostsField,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManageHostsField {
    Name,
    Hostname,
    User,
}

pub(crate) enum HostsModalInputEvent {
    SetActiveField(ManageHostsField),
    MoveActiveField(bool),
    Edit(TextEditAction),
    ClearError,
}

#[derive(Debug, Clone)]
pub(crate) enum DeleteTarget {
    Worktree(usize),
    Outpost(usize),
    Repository(usize),
}

#[derive(Debug, Clone)]
pub(crate) struct DeleteModal {
    pub(crate) target: DeleteTarget,
    pub(crate) label: String,
    pub(crate) branch: String,
    pub(crate) has_unpushed: Option<bool>,
    pub(crate) delete_branch: bool,
    pub(crate) is_deleting: bool,
    pub(crate) error: Option<String>,
}

pub(crate) struct DaemonAuthModal {
    pub(crate) daemon_url: String,
    pub(crate) token: String,
    pub(crate) token_cursor: usize,
    pub(crate) error: Option<String>,
}

pub(crate) struct ConnectToHostModal {
    pub(crate) address: String,
    pub(crate) address_cursor: usize,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum TextEditAction {
    Insert(String),
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveHome,
    MoveEnd,
}

pub(crate) enum ConnectHostTarget {
    Http {
        url: String,
        auth_key: String,
    },
    Ssh {
        target: SshDaemonTarget,
        auth_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshDaemonTarget {
    pub(crate) user: Option<String>,
    pub(crate) host: String,
    pub(crate) ssh_port: u16,
    pub(crate) daemon_port: u16,
}

impl SshDaemonTarget {
    pub(crate) fn ssh_destination(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };

        match self.user.as_deref() {
            Some(user) if !user.trim().is_empty() => format!("{user}@{host}"),
            _ => host,
        }
    }
}

pub(crate) struct SshDaemonTunnel {
    pub(crate) child: Child,
    pub(crate) local_port: u16,
}

impl SshDaemonTunnel {
    pub(crate) fn start(target: &SshDaemonTarget) -> Result<Self, String> {
        let local_port = crate::reserve_local_loopback_port()?;
        let forward = format!("127.0.0.1:{local_port}:127.0.0.1:{}", target.daemon_port);

        let mut command = crate::create_command("ssh");
        command
            .arg("-N")
            .arg("-T")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ExitOnForwardFailure=yes")
            .arg("-o")
            .arg("ServerAliveInterval=15")
            .arg("-o")
            .arg("ServerAliveCountMax=3")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg("-L")
            .arg(forward)
            .arg("-p")
            .arg(target.ssh_port.to_string())
            .arg(target.ssh_destination())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = command.spawn().map_err(|error| {
            format!(
                "failed to launch ssh tunnel to {}: {error}",
                target.ssh_destination()
            )
        })?;

        Ok(Self { child, local_port })
    }

    pub(crate) fn local_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.local_port)
    }

    pub(crate) fn stop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Drop for SshDaemonTunnel {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) struct RepositoryContextMenu {
    pub(crate) repository_index: usize,
    pub(crate) position: gpui::Point<Pixels>,
}

pub(crate) struct WorktreeContextMenu {
    pub(crate) worktree_index: usize,
    pub(crate) position: gpui::Point<Pixels>,
}

pub(crate) struct OutpostContextMenu {
    pub(crate) outpost_index: usize,
    pub(crate) position: gpui::Point<Pixels>,
}

pub(crate) struct WorktreeHoverPopover {
    pub(crate) worktree_index: usize,
    /// Vertical position of the mouse when hover started (window coords).
    pub(crate) mouse_y: Pixels,
    pub(crate) checks_expanded: bool,
}

pub(crate) struct CreatedWorktree {
    pub(crate) worktree_name: String,
    pub(crate) branch_name: String,
    pub(crate) worktree_path: PathBuf,
    pub(crate) checkout_kind: CheckoutKind,
    pub(crate) source_repo_root: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct RemoteDaemonState {
    pub(crate) client: Arc<terminal_daemon_http::HttpTerminalDaemon>,
    pub(crate) hostname: String,
    pub(crate) repositories: Vec<terminal_daemon_http::RemoteRepositoryDto>,
    pub(crate) worktrees: Vec<terminal_daemon_http::RemoteWorktreeDto>,
    pub(crate) loading: bool,
    pub(crate) expanded: bool,
    pub(crate) error: Option<String>,
}

/// Tracks which remote worktree is currently selected in the sidebar,
/// without switching the window's primary daemon connection.
#[derive(Debug, Clone)]
pub(crate) struct ActiveRemoteWorktree {
    pub(crate) daemon_index: usize,
    pub(crate) worktree_path: PathBuf,
}
