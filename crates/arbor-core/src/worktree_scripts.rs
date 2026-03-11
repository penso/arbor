use {
    crate::repo_config,
    std::{
        path::{Path, PathBuf},
        process::Command,
    },
    thiserror::Error,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeScriptPhase {
    Setup,
    Teardown,
}

impl WorktreeScriptPhase {
    fn label(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Teardown => "teardown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorktreeScriptContext {
    pub repo_path: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: Option<String>,
}

impl WorktreeScriptContext {
    pub fn new(repo_path: &Path, worktree_path: &Path, branch: Option<&str>) -> Self {
        Self {
            repo_path: repo_path.to_path_buf(),
            worktree_path: worktree_path.to_path_buf(),
            branch: branch
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        }
    }
}

#[derive(Debug, Error)]
pub enum WorktreeScriptError {
    #[error(transparent)]
    RepoConfig(Box<repo_config::RepoConfigError>),
    #[error("{message}")]
    CommandFailed { message: String },
}

impl From<repo_config::RepoConfigError> for WorktreeScriptError {
    fn from(error: repo_config::RepoConfigError) -> Self {
        Self::RepoConfig(Box::new(error))
    }
}

pub fn run_worktree_scripts(
    repo_root: &Path,
    phase: WorktreeScriptPhase,
    context: &WorktreeScriptContext,
) -> Result<(), WorktreeScriptError> {
    let Some(config) = repo_config::read_repo_config(repo_root)? else {
        return Ok(());
    };

    let commands = match phase {
        WorktreeScriptPhase::Setup => config.scripts.setup,
        WorktreeScriptPhase::Teardown => config.scripts.teardown,
    };

    for command in commands {
        run_command(repo_root, phase, context, &command)?;
    }

    Ok(())
}

fn run_command(
    repo_root: &Path,
    phase: WorktreeScriptPhase,
    context: &WorktreeScriptContext,
    command: &str,
) -> Result<(), WorktreeScriptError> {
    if command.trim().is_empty() {
        return Ok(());
    }

    let mut process = shell_command(command);
    process.current_dir(repo_root);
    process.env("ARBOR_WORKTREE_PATH", &context.worktree_path);
    process.env("ARBOR_REPO_PATH", &context.repo_path);
    process.env(
        "ARBOR_BRANCH",
        context.branch.as_deref().unwrap_or_default(),
    );

    let output = process
        .output()
        .map_err(|error| WorktreeScriptError::CommandFailed {
            message: format!(
                "{} script failed in `{}`: `{}` (failed to spawn command: {error})",
                phase.label(),
                repo_root.display(),
                command
            ),
        })?;

    if output.status.success() {
        return Ok(());
    }

    let details = match output.status.code() {
        Some(code) => format!("exit code {code}"),
        None => "terminated by signal".to_owned(),
    };

    let stdout = format_stream("stdout", non_empty_utf8(output.stdout).as_deref());
    let stderr = format_stream("stderr", non_empty_utf8(output.stderr).as_deref());
    Err(WorktreeScriptError::CommandFailed {
        message: format!(
            "{} script failed in `{}`: `{}` ({details}){stdout}{stderr}",
            phase.label(),
            repo_root.display(),
            command,
        ),
    })
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

fn non_empty_utf8(bytes: Vec<u8>) -> Option<String> {
    let value = String::from_utf8_lossy(&bytes).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn format_stream(label: &str, value: Option<&str>) -> String {
    match value {
        Some(text) if !text.is_empty() => format!("\n{label}:\n{text}"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "windows"))]
    use super::{WorktreeScriptContext, WorktreeScriptPhase, run_worktree_scripts};
    #[cfg(not(target_os = "windows"))]
    use {std::fs, tempfile::tempdir};

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn setup_script_receives_expected_environment() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let repo_root = dir.path();
        let output_path = repo_root.join("hook-output.txt");
        let escaped_output = output_path.to_string_lossy().replace('"', "\\\"");
        let content = format!(
            r#"[scripts]
setup = ["printf '%s|%s|%s' \"$ARBOR_WORKTREE_PATH\" \"$ARBOR_REPO_PATH\" \"$ARBOR_BRANCH\" > \"{escaped_output}\""]
"#
        );
        if let Err(error) = fs::write(repo_root.join("arbor.toml"), content) {
            panic!("failed to write config: {error}");
        }

        let context =
            WorktreeScriptContext::new(repo_root, &repo_root.join("feature-wt"), Some("feature/a"));

        if let Err(error) = run_worktree_scripts(repo_root, WorktreeScriptPhase::Setup, &context) {
            panic!("script should succeed: {error}");
        }

        let written = match fs::read_to_string(output_path) {
            Ok(value) => value,
            Err(error) => panic!("failed to read output: {error}"),
        };
        let expected = format!(
            "{}|{}|feature/a",
            context.worktree_path.display(),
            context.repo_path.display()
        );
        assert_eq!(written, expected);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn setup_scripts_are_loaded_from_repository_root_config() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let repo_root = dir.path();
        let worktree_path = repo_root.join("feature-wt");
        if let Err(error) = fs::create_dir_all(&worktree_path) {
            panic!("failed to create worktree dir: {error}");
        }

        let output_path = repo_root.join("scope-output.txt");
        let escaped_output = output_path.to_string_lossy().replace('"', "\\\"");
        let repo_content = format!(
            r#"[scripts]
setup = ["printf 'repo-root' > \"{escaped_output}\""]
"#
        );
        if let Err(error) = fs::write(repo_root.join("arbor.toml"), repo_content) {
            panic!("failed to write repo config: {error}");
        }

        let worktree_content = format!(
            r#"[scripts]
setup = ["printf 'worktree-local' > \"{escaped_output}\""]
"#
        );
        if let Err(error) = fs::write(worktree_path.join("arbor.toml"), worktree_content) {
            panic!("failed to write worktree config: {error}");
        }

        let context = WorktreeScriptContext::new(repo_root, &worktree_path, Some("feature/a"));
        if let Err(error) = run_worktree_scripts(repo_root, WorktreeScriptPhase::Setup, &context) {
            panic!("script should succeed: {error}");
        }

        let written = match fs::read_to_string(output_path) {
            Ok(value) => value,
            Err(error) => panic!("failed to read output: {error}"),
        };
        assert_eq!(written, "repo-root");
    }
}
