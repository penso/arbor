use {
    crate::domain::Issue,
    minijinja::{Environment, UndefinedBehavior, context},
    serde::{Deserialize, Serialize},
    serde_json::Value,
    std::{
        collections::HashMap,
        env, fs,
        path::{Path, PathBuf},
        time::SystemTime,
    },
    thiserror::Error,
};

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("missing_workflow_file: {0}")]
    MissingWorkflowFile(PathBuf),
    #[error("workflow_parse_error: {0}")]
    WorkflowParse(String),
    #[error("workflow_front_matter_not_a_map")]
    WorkflowFrontMatterNotMap,
    #[error("template_parse_error: {0}")]
    TemplateParse(String),
    #[error("template_render_error: {0}")]
    TemplateRender(String),
    #[error("invalid_config: {0}")]
    InvalidConfig(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowDefinition {
    pub config: Value,
    pub prompt_template: String,
}

impl WorkflowDefinition {
    pub fn render_prompt(
        &self,
        issue: &Issue,
        attempt: Option<u32>,
    ) -> Result<String, WorkflowError> {
        let template_source = if self.prompt_template.trim().is_empty() {
            "You are working on an issue from Linear."
        } else {
            self.prompt_template.as_str()
        };

        let mut environment = Environment::new();
        environment.set_undefined_behavior(UndefinedBehavior::Strict);
        environment
            .add_template("workflow", template_source)
            .map_err(|error| WorkflowError::TemplateParse(error.to_string()))?;

        let template = environment
            .get_template("workflow")
            .map_err(|error| WorkflowError::TemplateParse(error.to_string()))?;
        template
            .render(context!(issue => issue, attempt => attempt))
            .map_err(|error| WorkflowError::TemplateRender(error.to_string()))
    }
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowLoader {
    pub path: PathBuf,
    pub last_modified: Option<SystemTime>,
}

impl WorkflowLoader {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            last_modified: None,
        }
    }

    pub fn load(&mut self) -> Result<WorkflowDefinition, WorkflowError> {
        let path = &self.path;
        if !path.exists() {
            return Err(WorkflowError::MissingWorkflowFile(path.clone()));
        }

        let metadata = fs::metadata(path)?;
        self.last_modified = metadata.modified().ok();

        let content = fs::read_to_string(path)?;
        parse_workflow(&content)
    }

    pub fn load_if_changed(&mut self) -> Result<Option<WorkflowDefinition>, WorkflowError> {
        let metadata = fs::metadata(&self.path)?;
        let modified = metadata.modified().ok();
        if modified.is_some() && modified == self.last_modified {
            return Ok(None);
        }

        self.load().map(Some)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookScripts {
    pub after_create: Option<String>,
    pub before_run: Option<String>,
    pub after_run: Option<String>,
    pub before_remove: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceConfig {
    pub tracker: TrackerConfig,
    pub polling: PollingConfig,
    pub workspace: WorkspaceConfig,
    pub hooks: HookScripts,
    pub agent: AgentConfig,
    pub codex: CodexConfig,
    pub server: SymphonyServerConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackerConfig {
    pub kind: String,
    pub endpoint: String,
    pub api_key: String,
    pub project_slug: String,
    pub active_states: Vec<String>,
    pub terminal_states: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PollingConfig {
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentConfig {
    pub max_concurrent_agents: usize,
    pub max_retry_backoff_ms: u64,
    pub max_turns: u32,
    pub max_concurrent_agents_by_state: HashMap<String, usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexConfig {
    pub command: String,
    pub approval_policy: Option<String>,
    pub thread_sandbox: Option<String>,
    pub turn_sandbox_policy: Option<Value>,
    pub turn_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub stall_timeout_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymphonyServerConfig {
    pub port: Option<u16>,
}

pub type TypedWorkflowConfig = ServiceConfig;

pub fn default_workflow_path(cwd: &Path) -> PathBuf {
    cwd.join("WORKFLOW.md")
}

pub fn parse_workflow(content: &str) -> Result<WorkflowDefinition, WorkflowError> {
    let (config, body) = split_front_matter(content)?;
    Ok(WorkflowDefinition {
        config,
        prompt_template: body.trim().to_owned(),
    })
}

pub fn resolve_config(
    definition: &WorkflowDefinition,
) -> Result<TypedWorkflowConfig, WorkflowError> {
    let root = definition
        .config
        .as_object()
        .ok_or(WorkflowError::WorkflowFrontMatterNotMap)?;
    let tracker = root.get("tracker");
    let polling = root.get("polling");
    let workspace = root.get("workspace");
    let hooks = root.get("hooks");
    let agent = root.get("agent");
    let codex = root.get("codex");
    let server = root.get("server");

    let tracker_kind = get_string(tracker, "kind").unwrap_or_else(|| "linear".to_owned());
    if tracker_kind.trim().is_empty() {
        return Err(WorkflowError::InvalidConfig(
            "tracker.kind is required".to_owned(),
        ));
    }

    let tracker_api_key = resolve_env_token(
        get_string(tracker, "api_key").unwrap_or_else(|| "$LINEAR_API_KEY".to_owned()),
    );
    if tracker_api_key.trim().is_empty() {
        return Err(WorkflowError::InvalidConfig(
            "tracker.api_key is required".to_owned(),
        ));
    }

    let project_slug = get_string(tracker, "project_slug").unwrap_or_default();
    if tracker_kind == "linear" && project_slug.trim().is_empty() {
        return Err(WorkflowError::InvalidConfig(
            "tracker.project_slug is required".to_owned(),
        ));
    }

    let workspace_root = normalize_path(&get_string(workspace, "root").unwrap_or_else(|| {
        env::temp_dir()
            .join("symphony_workspaces")
            .display()
            .to_string()
    }));
    let hook_timeout_ms = positive_u64(get_value(hooks, "timeout_ms")).unwrap_or(60_000);
    let max_concurrent_agents =
        positive_usize(get_value(agent, "max_concurrent_agents")).unwrap_or(10);
    let max_retry_backoff_ms =
        positive_u64(get_value(agent, "max_retry_backoff_ms")).unwrap_or(300_000);
    let max_turns = positive_u32(get_value(agent, "max_turns")).unwrap_or(20);
    let interval_ms = positive_u64(get_value(polling, "interval_ms")).unwrap_or(30_000);
    let turn_timeout_ms = positive_u64(get_value(codex, "turn_timeout_ms")).unwrap_or(3_600_000);
    let read_timeout_ms = positive_u64(get_value(codex, "read_timeout_ms")).unwrap_or(5_000);
    let stall_timeout_ms = signed_i64(get_value(codex, "stall_timeout_ms")).unwrap_or(300_000);
    let codex_command =
        get_string(codex, "command").unwrap_or_else(|| "codex app-server".to_owned());
    if codex_command.trim().is_empty() {
        return Err(WorkflowError::InvalidConfig(
            "codex.command is required".to_owned(),
        ));
    }

    Ok(ServiceConfig {
        tracker: TrackerConfig {
            kind: tracker_kind,
            endpoint: get_string(tracker, "endpoint")
                .unwrap_or_else(|| "https://api.linear.app/graphql".to_owned()),
            api_key: tracker_api_key,
            project_slug,
            active_states: get_string_list(tracker, "active_states")
                .unwrap_or_else(|| vec!["Todo".to_owned(), "In Progress".to_owned()]),
            terminal_states: get_string_list(tracker, "terminal_states").unwrap_or_else(|| {
                vec![
                    "Closed".to_owned(),
                    "Cancelled".to_owned(),
                    "Canceled".to_owned(),
                    "Duplicate".to_owned(),
                    "Done".to_owned(),
                ]
            }),
        },
        polling: PollingConfig { interval_ms },
        workspace: WorkspaceConfig {
            root: workspace_root,
        },
        hooks: HookScripts {
            after_create: get_string(hooks, "after_create"),
            before_run: get_string(hooks, "before_run"),
            after_run: get_string(hooks, "after_run"),
            before_remove: get_string(hooks, "before_remove"),
            timeout_ms: hook_timeout_ms,
        },
        agent: AgentConfig {
            max_concurrent_agents,
            max_retry_backoff_ms,
            max_turns,
            max_concurrent_agents_by_state: get_state_limit_map(
                agent,
                "max_concurrent_agents_by_state",
            ),
        },
        codex: CodexConfig {
            command: codex_command,
            approval_policy: get_string(codex, "approval_policy"),
            thread_sandbox: get_string(codex, "thread_sandbox"),
            turn_sandbox_policy: codex
                .and_then(|value| value.get("turn_sandbox_policy"))
                .cloned(),
            turn_timeout_ms,
            read_timeout_ms,
            stall_timeout_ms,
        },
        server: SymphonyServerConfig {
            port: positive_u16(server.and_then(|value| value.get("port"))),
        },
    })
}

fn split_front_matter(content: &str) -> Result<(Value, String), WorkflowError> {
    if !content.starts_with("---") {
        return Ok((Value::Object(Default::default()), content.to_owned()));
    }

    let mut lines = content.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Ok((Value::Object(Default::default()), content.to_owned()));
    }

    let mut yaml_lines = Vec::new();
    let mut body_lines = Vec::new();
    let mut in_yaml = true;
    for line in lines {
        if in_yaml && line.trim() == "---" {
            in_yaml = false;
            continue;
        }

        if in_yaml {
            yaml_lines.push(line);
        } else {
            body_lines.push(line);
        }
    }

    if in_yaml {
        return Err(WorkflowError::WorkflowParse(
            "front matter missing closing delimiter".to_owned(),
        ));
    }

    let yaml = yaml_lines.join("\n");
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml)
        .map_err(|error| WorkflowError::WorkflowParse(error.to_string()))?;
    let value = serde_json::to_value(value)
        .map_err(|error| WorkflowError::WorkflowParse(error.to_string()))?;
    if !value.is_object() {
        return Err(WorkflowError::WorkflowFrontMatterNotMap);
    }

    Ok((value, body_lines.join("\n")))
}

fn get_value<'a>(parent: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    parent.and_then(|value| value.get(key))
}

fn get_string(parent: Option<&Value>, key: &str) -> Option<String> {
    get_value(parent, key).and_then(|value| match value {
        Value::String(value) => Some(value.trim().to_owned()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

fn get_string_list(parent: Option<&Value>, key: &str) -> Option<Vec<String>> {
    get_value(parent, key).and_then(|value| {
        value.as_array().map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|value| value.trim().to_owned()))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
    })
}

fn get_state_limit_map(parent: Option<&Value>, key: &str) -> HashMap<String, usize> {
    let mut values = HashMap::new();
    let Some(map) = get_value(parent, key).and_then(Value::as_object) else {
        return values;
    };

    for (state, value) in map {
        if let Some(limit) = positive_usize(Some(value)) {
            values.insert(state.to_ascii_lowercase(), limit);
        }
    }

    values
}

fn resolve_env_token(value: String) -> String {
    if let Some(name) = value.strip_prefix('$') {
        env::var(name).unwrap_or_default().trim().to_owned()
    } else {
        value
    }
}

fn normalize_path(raw: &str) -> PathBuf {
    let expanded = resolve_env_token(raw.trim().to_owned());
    if let Some(home_relative) = expanded.strip_prefix("~/")
        && let Ok(home) = env::var("HOME")
    {
        return PathBuf::from(home).join(home_relative);
    }

    PathBuf::from(expanded)
}

fn positive_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| match value {
        Value::Number(number) => number.as_u64(),
        Value::String(string) => string.trim().parse::<u64>().ok(),
        _ => None,
    })
}

fn positive_usize(value: Option<&Value>) -> Option<usize> {
    positive_u64(value).and_then(|value| usize::try_from(value).ok())
}

fn positive_u32(value: Option<&Value>) -> Option<u32> {
    positive_u64(value).and_then(|value| u32::try_from(value).ok())
}

fn positive_u16(value: Option<&Value>) -> Option<u16> {
    positive_u64(value).and_then(|value| u16::try_from(value).ok())
}

fn signed_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| match value {
        Value::Number(number) => number.as_i64(),
        Value::String(string) => string.trim().parse::<i64>().ok(),
        _ => None,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_front_matter_and_body() {
        let workflow = parse_workflow(
            "---\ntracker:\n  kind: linear\n  api_key: foo\n  project_slug: arbor\n---\nhello {{ issue.identifier }}",
        )
        .expect("workflow should parse");

        assert_eq!(workflow.prompt_template, "hello {{ issue.identifier }}");
        assert_eq!(workflow.config["tracker"]["kind"], "linear");
    }

    #[test]
    fn rejects_non_map_front_matter() {
        let error = parse_workflow("---\n- foo\n---\nbody").expect_err("should fail");
        assert!(matches!(error, WorkflowError::WorkflowFrontMatterNotMap));
    }

    #[test]
    fn renders_strict_template() {
        let workflow = parse_workflow("hello {{ issue.identifier }} / {{ attempt }}")
            .expect("workflow should parse");
        let issue = Issue {
            id: "1".to_owned(),
            identifier: "ARB-1".to_owned(),
            title: "Test".to_owned(),
            state: "Todo".to_owned(),
            ..Issue::default()
        };

        let rendered = workflow
            .render_prompt(&issue, Some(2))
            .expect("render should succeed");
        assert_eq!(rendered, "hello ARB-1 / 2");
    }

    #[test]
    fn resolves_defaults() {
        let workflow = WorkflowDefinition {
            config: serde_json::json!({
                "tracker": {
                    "kind": "linear",
                    "api_key": "token",
                    "project_slug": "arbor"
                }
            }),
            prompt_template: String::new(),
        };

        let config = resolve_config(&workflow).expect("config should resolve");
        assert_eq!(config.polling.interval_ms, 30_000);
        assert_eq!(config.agent.max_turns, 20);
        assert_eq!(config.codex.command, "codex app-server");
    }
}
