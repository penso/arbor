use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptExecutionMode {
    CaptureOutput,
    TerminalSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptExecutionPlan {
    pub(crate) shell_command: String,
}

pub(crate) fn command_for_execution_mode(
    preset: AgentPresetKind,
    configured_command: &str,
    execution_mode: ExecutionMode,
) -> Result<String, PromptError> {
    let configured_command = configured_command.trim();
    if configured_command.is_empty() {
        return Err(PromptError::Execution(format!(
            "{} preset command is empty",
            preset.label()
        )));
    }

    let mut tokens = shlex::split(configured_command).ok_or_else(|| {
        PromptError::Execution(format!("failed to parse {} preset command", preset.label()))
    })?;
    strip_execution_mode_flags(preset, &mut tokens);

    match preset {
        AgentPresetKind::Claude => match execution_mode {
            ExecutionMode::Plan => {
                tokens.push("--permission-mode".to_owned());
                tokens.push("plan".to_owned());
            },
            ExecutionMode::Build => {
                tokens.push("--permission-mode".to_owned());
                tokens.push("acceptEdits".to_owned());
            },
            ExecutionMode::Yolo => {
                tokens.push("--dangerously-skip-permissions".to_owned());
            },
        },
        AgentPresetKind::Codex => match execution_mode {
            ExecutionMode::Plan => {
                tokens.push("-a".to_owned());
                tokens.push("on-request".to_owned());
                tokens.push("-s".to_owned());
                tokens.push("read-only".to_owned());
            },
            ExecutionMode::Build => {
                tokens.push("--full-auto".to_owned());
            },
            ExecutionMode::Yolo => {
                tokens.push("--dangerously-bypass-approvals-and-sandbox".to_owned());
            },
        },
        AgentPresetKind::Copilot => match execution_mode {
            ExecutionMode::Plan => {},
            ExecutionMode::Build => {
                tokens.push("--allow-all-tools".to_owned());
            },
            ExecutionMode::Yolo => {
                tokens.push("--yolo".to_owned());
            },
        },
        AgentPresetKind::Pi | AgentPresetKind::OpenCode => {},
    }

    Ok(join_shell_tokens(&tokens))
}

pub(crate) fn build_prompt_execution_plan(
    preset: AgentPresetKind,
    configured_command: &str,
    prompt: &str,
    execution_mode: ExecutionMode,
    mode: PromptExecutionMode,
) -> Result<PromptExecutionPlan, PromptError> {
    let configured_command =
        command_for_execution_mode(preset, configured_command, execution_mode)?;
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(PromptError::Execution("prompt cannot be empty".to_owned()));
    }

    let shell_command = match mode {
        PromptExecutionMode::CaptureOutput => match preset {
            AgentPresetKind::Claude => {
                format!("{configured_command} --print {}", shell_quote(prompt))
            },
            AgentPresetKind::Codex => {
                format!("{configured_command} exec {}", shell_quote(prompt))
            },
            AgentPresetKind::OpenCode => {
                format!("{configured_command} run {}", shell_quote(prompt))
            },
            AgentPresetKind::Copilot => {
                format!("{configured_command} -p {} -s", shell_quote(prompt))
            },
            AgentPresetKind::Pi => {
                return Err(PromptError::Execution(format!(
                    "{} does not support non-interactive prompt execution yet",
                    preset.label()
                )));
            },
        },
        PromptExecutionMode::TerminalSession => {
            format!("{configured_command} {}", shell_quote(prompt))
        },
    };

    Ok(PromptExecutionPlan { shell_command })
}

pub(crate) fn run_prompt_capture(
    worktree_path: &Path,
    preset: AgentPresetKind,
    configured_command: &str,
    prompt: &str,
    execution_mode: ExecutionMode,
    operation: &str,
) -> Result<String, PromptError> {
    let plan = build_prompt_execution_plan(
        preset,
        configured_command,
        prompt,
        execution_mode,
        PromptExecutionMode::CaptureOutput,
    )?;
    let mut command = shell_expression_command(&plan.shell_command);
    command.current_dir(worktree_path);

    let output = run_command_output(&mut command, operation)
        .map_err(|error| PromptError::Execution(error.to_string()))?;
    if !output.status.success() {
        return Err(PromptError::Execution(command_failure_message(
            operation, &output,
        )));
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() {
        return Err(PromptError::Execution(format!(
            "{operation} returned empty output"
        )));
    }

    Ok(text)
}

pub(crate) fn prompt_terminal_invocation(
    preset: AgentPresetKind,
    configured_command: &str,
    prompt: &str,
    execution_mode: ExecutionMode,
) -> Result<String, PromptError> {
    build_prompt_execution_plan(
        preset,
        configured_command,
        prompt,
        execution_mode,
        PromptExecutionMode::TerminalSession,
    )
    .map(|plan| plan.shell_command)
}

pub(crate) fn strip_execution_mode_flags(preset: AgentPresetKind, tokens: &mut Vec<String>) {
    match preset {
        AgentPresetKind::Claude => {
            *tokens = strip_tokens(tokens, &[
                ("--permission-mode", true),
                ("--dangerously-skip-permissions", false),
                ("--allow-dangerously-skip-permissions", false),
            ]);
        },
        AgentPresetKind::Codex => {
            *tokens = strip_tokens(tokens, &[
                ("--dangerously-bypass-approvals-and-sandbox", false),
                ("--full-auto", false),
                ("-a", true),
                ("--ask-for-approval", true),
                ("-s", true),
                ("--sandbox", true),
            ]);
        },
        AgentPresetKind::Copilot => {
            *tokens = strip_tokens(tokens, &[
                ("--yolo", false),
                ("--allow-all", false),
                ("--allow-all-tools", false),
                ("--allow-all-paths", false),
                ("--allow-all-urls", false),
            ]);
        },
        AgentPresetKind::Pi | AgentPresetKind::OpenCode => {},
    }
}

pub(crate) fn strip_tokens(tokens: &[String], flags: &[(&str, bool)]) -> Vec<String> {
    let mut stripped = Vec::with_capacity(tokens.len());
    let mut index = 0usize;

    while let Some(token) = tokens.get(index) {
        let mut matched = false;
        for (flag, takes_value) in flags {
            if token == flag {
                matched = true;
                index += 1 + usize::from(*takes_value);
                break;
            }
            let inline = format!("{flag}=");
            if token.starts_with(&inline) {
                matched = true;
                index += 1;
                break;
            }
        }

        if matched {
            continue;
        }

        stripped.push(token.clone());
        index += 1;
    }

    stripped
}

pub(crate) fn join_shell_tokens(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|token| shell_quote(token))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(target_os = "windows")]
pub(crate) fn shell_expression_command(expression: &str) -> Command {
    let mut command = create_command("cmd");
    command.arg("/C").arg(expression);
    command
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn shell_expression_command(expression: &str) -> Command {
    let mut command = create_command("sh");
    command.arg("-lc").arg(expression);
    command
}

#[cfg(test)]
mod prompt_runner_tests {
    use super::*;

    #[test]
    fn codex_capture_plan_uses_exec_mode() {
        let plan = build_prompt_execution_plan(
            AgentPresetKind::Codex,
            "codex --model gpt-5",
            "summarize the diff",
            ExecutionMode::Build,
            PromptExecutionMode::CaptureOutput,
        )
        .unwrap_or_else(|error| panic!("plan should build: {error}"));

        let tokens = shlex::split(&plan.shell_command)
            .unwrap_or_else(|| panic!("shell command should split"));
        assert!(tokens.starts_with(&[
            "codex".to_owned(),
            "--model".to_owned(),
            "gpt-5".to_owned(),
            "--full-auto".to_owned(),
            "exec".to_owned(),
        ]));
    }

    #[test]
    fn pi_capture_plan_is_not_supported() {
        let error = build_prompt_execution_plan(
            AgentPresetKind::Pi,
            "pi",
            "summarize the diff",
            ExecutionMode::Build,
            PromptExecutionMode::CaptureOutput,
        )
        .err()
        .unwrap_or_else(|| panic!("pi capture should be unsupported"));

        assert!(
            error
                .to_string()
                .contains("Pi does not support non-interactive prompt execution yet")
        );
    }

    #[test]
    fn copilot_capture_plan_uses_prompt_and_silent_flags() {
        let plan = build_prompt_execution_plan(
            AgentPresetKind::Copilot,
            "copilot --model gpt-5.2",
            "summarize the diff",
            ExecutionMode::Build,
            PromptExecutionMode::CaptureOutput,
        )
        .unwrap_or_else(|error| panic!("plan should build: {error}"));

        let tokens = shlex::split(&plan.shell_command)
            .unwrap_or_else(|| panic!("shell command should split"));
        assert!(tokens.starts_with(&[
            "copilot".to_owned(),
            "--model".to_owned(),
            "gpt-5.2".to_owned(),
            "--allow-all-tools".to_owned(),
            "-p".to_owned(),
        ]));
        assert_eq!(tokens.last().map(String::as_str), Some("-s"));
    }

    #[test]
    fn terminal_plan_quotes_prompt() {
        let plan = build_prompt_execution_plan(
            AgentPresetKind::Claude,
            "claude --dangerously-skip-permissions",
            "review branch named it's-ready",
            ExecutionMode::Plan,
            PromptExecutionMode::TerminalSession,
        )
        .unwrap_or_else(|error| panic!("plan should build: {error}"));

        let tokens = shlex::split(&plan.shell_command)
            .unwrap_or_else(|| panic!("shell command should split"));
        assert!(tokens.starts_with(&[
            "claude".to_owned(),
            "--permission-mode".to_owned(),
            "plan".to_owned(),
        ]));
        assert_eq!(
            tokens.last().map(String::as_str),
            Some("review branch named it's-ready")
        );
    }

    #[test]
    fn yolo_mode_rewrites_codex_permissions() {
        let command = command_for_execution_mode(
            AgentPresetKind::Codex,
            "codex --full-auto -a on-request -s read-only",
            ExecutionMode::Yolo,
        )
        .unwrap_or_else(|error| panic!("command should build: {error}"));

        assert!(command.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(!command.contains("--full-auto"));
        assert!(!command.contains("read-only"));
    }
}
