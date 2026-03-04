use std::env;

use {
    arbor_core::worktree,
    gpui::{
        App, Application, Bounds, Context, Window, WindowBounds, WindowOptions, div, prelude::*,
        px, rgb, size,
    },
};

#[derive(Debug, Clone)]
struct WorktreeRow {
    path: String,
    branch: String,
    head: String,
    status: String,
}

struct ArborWindow {
    repo_root: String,
    worktrees: Vec<WorktreeRow>,
    error_message: Option<String>,
}

impl ArborWindow {
    fn load() -> Self {
        let cwd = match env::current_dir() {
            Ok(path) => path,
            Err(error) => {
                return Self {
                    repo_root: "<unknown>".to_owned(),
                    worktrees: Vec::new(),
                    error_message: Some(format!("failed to read current directory: {error}")),
                };
            },
        };

        let repo_root = match worktree::repo_root(&cwd) {
            Ok(path) => path,
            Err(error) => {
                return Self {
                    repo_root: cwd.display().to_string(),
                    worktrees: Vec::new(),
                    error_message: Some(format!("failed to resolve git repository root: {error}")),
                };
            },
        };

        match worktree::list(&repo_root) {
            Ok(entries) => Self {
                repo_root: repo_root.display().to_string(),
                worktrees: entries.iter().map(WorktreeRow::from_worktree).collect(),
                error_message: None,
            },
            Err(error) => Self {
                repo_root: repo_root.display().to_string(),
                worktrees: Vec::new(),
                error_message: Some(format!("failed to load worktrees: {error}")),
            },
        }
    }
}

impl WorktreeRow {
    fn from_worktree(entry: &worktree::Worktree) -> Self {
        let mut status_bits = Vec::new();

        if entry.is_bare {
            status_bits.push("bare".to_owned());
        }

        if entry.is_detached {
            status_bits.push("detached".to_owned());
        }

        if let Some(reason) = &entry.lock_reason {
            status_bits.push(format!("locked ({reason})"));
        }

        if let Some(reason) = &entry.prune_reason {
            status_bits.push(format!("prunable ({reason})"));
        }

        if status_bits.is_empty() {
            status_bits.push("clean".to_owned());
        }

        let branch = entry
            .branch
            .as_deref()
            .map(short_branch)
            .unwrap_or_else(|| "-".to_owned());

        let head = entry
            .head
            .as_deref()
            .map(short_head)
            .unwrap_or_else(|| "-".to_owned());

        Self {
            path: entry.path.display().to_string(),
            branch,
            head,
            status: status_bits.join(", "),
        }
    }
}

impl Render for ArborWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let show_error = self.error_message.is_some();
        let error_message = self.error_message.clone().unwrap_or_default();
        let has_worktrees = !self.worktrees.is_empty();

        div()
            .size_full()
            .p_4()
            .bg(rgb(0x0f141b))
            .text_color(rgb(0xe8edf2))
            .font_family(".ZedMono")
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_2xl().font_semibold().child("Arbor"))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa2adbb))
                    .child(format!("Repository: {}", self.repo_root)),
            )
            .child(div().when(show_error, |this| {
                this.px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(0x5e2e2f))
                    .text_color(rgb(0xffe0df))
                    .child(error_message)
            }))
            .child(div().when(!has_worktrees, |this| {
                this.mt_4()
                    .text_color(rgb(0xa2adbb))
                    .text_sm()
                    .child("No worktrees found")
            }))
            .child(
                div().flex().flex_col().gap_2().children(
                    self.worktrees
                        .iter()
                        .map(|row| render_worktree_row(row.clone())),
                ),
            )
    }
}

fn render_worktree_row(row: WorktreeRow) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(0x2b3544))
        .bg(rgb(0x161d26))
        .p_3()
        .child(div().text_sm().font_semibold().child(row.path))
        .child(div().text_xs().text_color(rgb(0xa2adbb)).child(format!(
            "branch: {}    head: {}    status: {}",
            row.branch, row.head, row.status,
        )))
}

fn short_branch(value: &str) -> String {
    value
        .strip_prefix("refs/heads/")
        .unwrap_or(value)
        .to_owned()
}

fn short_head(value: &str) -> String {
    value.chars().take(8).collect()
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(960.), px(720.)), cx);

        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| ArborWindow::load()),
        ) {
            eprintln!("failed to open Arbor window: {error:#}");
            cx.quit();
            return;
        }

        cx.activate(true);
    });
}
