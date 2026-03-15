use {
    crate::workflow::HookScripts,
    std::{
        fs,
        path::{Path, PathBuf},
        process::Stdio,
    },
    thiserror::Error,
    tokio::{process::Command, time::Duration},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub path: PathBuf,
    pub workspace_key: String,
    pub created_now: bool,
}

#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    root: PathBuf,
    hooks: HookScripts,
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace path escaped root: {0}")]
    PathEscape(PathBuf),
    #[error("workspace path exists and is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("failed to create workspace: {0}")]
    Create(#[from] std::io::Error),
    #[error("workspace hook failed: {0}")]
    Hook(String),
}

impl WorkspaceManager {
    pub fn new(root: PathBuf, hooks: HookScripts) -> Self {
        Self { root, hooks }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workspace_path_for(&self, issue_identifier: &str) -> Result<PathBuf, WorkspaceError> {
        let key = sanitize_workspace_key(issue_identifier);
        let path = self.root.join(key);
        ensure_within_root(&self.root, &path)?;
        Ok(path)
    }

    pub async fn ensure_workspace(
        &self,
        issue_identifier: &str,
    ) -> Result<Workspace, WorkspaceError> {
        let workspace_key = sanitize_workspace_key(issue_identifier);
        let path = self.root.join(&workspace_key);
        ensure_within_root(&self.root, &path)?;

        if path.exists() && !path.is_dir() {
            return Err(WorkspaceError::NotDirectory(path));
        }

        let created_now = !path.exists();
        fs::create_dir_all(&path)?;

        let workspace = Workspace {
            path,
            workspace_key,
            created_now,
        };

        if workspace.created_now {
            self.run_hook(
                "after_create",
                self.hooks.after_create.as_deref(),
                &workspace.path,
            )
            .await?;
        }

        Ok(workspace)
    }

    pub async fn before_run(&self, workspace: &Workspace) -> Result<(), WorkspaceError> {
        self.run_hook(
            "before_run",
            self.hooks.before_run.as_deref(),
            &workspace.path,
        )
        .await
    }

    pub async fn after_run_best_effort(&self, workspace: &Workspace) {
        if let Err(error) = self
            .run_hook(
                "after_run",
                self.hooks.after_run.as_deref(),
                &workspace.path,
            )
            .await
        {
            tracing::warn!(%error, path = %workspace.path.display(), "workspace after_run hook failed");
        }
    }

    pub async fn remove_workspace(&self, issue_identifier: &str) -> Result<(), WorkspaceError> {
        let path = self.workspace_path_for(issue_identifier)?;
        if !path.exists() {
            return Ok(());
        }

        if let Err(error) = self
            .run_hook("before_remove", self.hooks.before_remove.as_deref(), &path)
            .await
        {
            tracing::warn!(%error, path = %path.display(), "workspace before_remove hook failed");
        }

        fs::remove_dir_all(&path)?;
        Ok(())
    }

    async fn run_hook(
        &self,
        hook_name: &str,
        script: Option<&str>,
        cwd: &Path,
    ) -> Result<(), WorkspaceError> {
        let Some(script) = script.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(());
        };

        tracing::info!(hook = hook_name, cwd = %cwd.display(), "running workspace hook");
        let mut command = shell_command(script);
        command
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let timeout = Duration::from_millis(self.hooks.timeout_ms.max(1));
        let child = command
            .spawn()
            .map_err(|error| WorkspaceError::Hook(format!("{hook_name}: {error}")))?;
        let output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| WorkspaceError::Hook(format!("{hook_name} timed out")))?;
        let output =
            output.map_err(|error| WorkspaceError::Hook(format!("{hook_name}: {error}")))?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = truncate_hook_output(&String::from_utf8_lossy(&output.stderr));
        let stdout = truncate_hook_output(&String::from_utf8_lossy(&output.stdout));
        Err(WorkspaceError::Hook(format!(
            "{hook_name} failed (status={} stdout=`{stdout}` stderr=`{stderr}`)",
            output.status
        )))
    }
}

pub fn sanitize_workspace_key(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn ensure_within_root(root: &Path, candidate: &Path) -> Result<(), WorkspaceError> {
    let absolute_root = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(root)
    };
    let absolute_candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(candidate)
    };

    if absolute_candidate.starts_with(&absolute_root) {
        Ok(())
    } else {
        Err(WorkspaceError::PathEscape(absolute_candidate))
    }
}

fn truncate_hook_output(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 240 {
        return trimmed.to_owned();
    }

    format!("{}...", &trimmed[..240])
}

#[cfg(target_os = "windows")]
fn shell_command(command: &str) -> Command {
    let mut process = Command::new("cmd");
    process.arg("/C").arg(command);
    process
}

#[cfg(not(target_os = "windows"))]
fn shell_command(command: &str) -> Command {
    let mut process = Command::new("sh");
    process.arg("-lc").arg(command);
    process
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    fn create_marker_command(marker_name: &str) -> String {
        format!("type nul > {marker_name}")
    }

    #[cfg(not(target_os = "windows"))]
    fn create_marker_command(marker_name: &str) -> String {
        format!(": > {marker_name}")
    }

    #[test]
    fn sanitizes_workspace_keys() {
        assert_eq!(sanitize_workspace_key("ABC-123"), "ABC-123");
        assert_eq!(sanitize_workspace_key("ABC/123 wow"), "ABC_123_wow");
    }

    #[tokio::test]
    async fn creates_workspace_and_runs_hook() {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker_name = "marker.txt";
        let manager = WorkspaceManager::new(temp.path().to_path_buf(), HookScripts {
            after_create: Some(create_marker_command(marker_name)),
            timeout_ms: 5_000,
            ..HookScripts::default()
        });

        let workspace = manager.ensure_workspace("ARB-1").await.expect("workspace");
        assert!(workspace.path.exists());
        let marker = workspace.path.join(marker_name);
        assert!(marker.exists());
    }
}
