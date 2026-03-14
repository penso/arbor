mod prompts;
mod resources;
mod tools;

pub use tools::{
    ActionStatus, AgentActivityOutput, ChangedFilesOutput, ChangesInput, CommitInput,
    ProcessNameInput, ProcessesOutput, PushInput, RepositoriesOutput, TaskHistoryOutput,
    TaskNameInput, TasksOutput, TerminalReadInput, TerminalResizeInput, TerminalSignalInput,
    TerminalTargetInput, TerminalWriteInput, TerminalsOutput, WorktreeListInput, WorktreesOutput,
};
use {
    arbor_core::task::{TaskExecution, TaskInfo},
    arbor_daemon_client::{
        AgentSessionDto, ChangedFileDto, CommitWorktreeRequest, CreateTerminalRequest,
        CreateTerminalResponse, CreateWorktreeRequest, DaemonClient, DaemonClientError,
        DeleteWorktreeRequest, GitActionResponse, HealthResponse, PushWorktreeRequest,
        RepositoryDto, TerminalResizeRequest, TerminalSignalRequest, WorktreeDto,
        WorktreeMutationResponse,
    },
    rmcp::{
        ErrorData, RoleServer, ServerHandler, ServiceExt,
        handler::server::router::tool::ToolRouter,
        model::{
            GetPromptRequestParams, GetPromptResult, Implementation, ListPromptsResult,
            ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
            ReadResourceRequestParams, ReadResourceResult, ServerCapabilities, ServerInfo,
        },
        service::RequestContext,
        tool_handler,
    },
    std::{future::Future, sync::Arc},
};

pub trait DaemonApi: Send + Sync {
    fn health(&self) -> Result<HealthResponse, DaemonClientError>;
    fn list_repositories(&self) -> Result<Vec<RepositoryDto>, DaemonClientError>;
    fn list_worktrees(
        &self,
        repo_root: Option<&str>,
    ) -> Result<Vec<WorktreeDto>, DaemonClientError>;
    fn create_worktree(
        &self,
        request: &CreateWorktreeRequest,
    ) -> Result<WorktreeMutationResponse, DaemonClientError>;
    fn delete_worktree(
        &self,
        request: &DeleteWorktreeRequest,
    ) -> Result<WorktreeMutationResponse, DaemonClientError>;
    fn list_changed_files(&self, path: &str) -> Result<Vec<ChangedFileDto>, DaemonClientError>;
    fn commit_worktree(
        &self,
        request: &CommitWorktreeRequest,
    ) -> Result<GitActionResponse, DaemonClientError>;
    fn push_worktree(
        &self,
        request: &PushWorktreeRequest,
    ) -> Result<GitActionResponse, DaemonClientError>;
    fn list_terminals(
        &self,
    ) -> Result<Vec<arbor_core::daemon::DaemonSessionRecord>, DaemonClientError>;
    fn create_terminal(
        &self,
        request: &CreateTerminalRequest,
    ) -> Result<CreateTerminalResponse, DaemonClientError>;
    fn read_terminal_output(
        &self,
        session_id: &str,
        max_lines: Option<usize>,
    ) -> Result<arbor_core::daemon::TerminalSnapshot, DaemonClientError>;
    fn write_terminal_input(&self, session_id: &str, data: &[u8]) -> Result<(), DaemonClientError>;
    fn resize_terminal(
        &self,
        session_id: &str,
        request: &TerminalResizeRequest,
    ) -> Result<(), DaemonClientError>;
    fn signal_terminal(
        &self,
        session_id: &str,
        request: &TerminalSignalRequest,
    ) -> Result<(), DaemonClientError>;
    fn detach_terminal(&self, session_id: &str) -> Result<(), DaemonClientError>;
    fn kill_terminal(&self, session_id: &str) -> Result<(), DaemonClientError>;
    fn list_agent_activity(&self) -> Result<Vec<AgentSessionDto>, DaemonClientError>;
    fn list_processes(&self) -> Result<Vec<arbor_core::process::ProcessInfo>, DaemonClientError>;
    fn start_all_processes(
        &self,
    ) -> Result<Vec<arbor_core::process::ProcessInfo>, DaemonClientError>;
    fn stop_all_processes(
        &self,
    ) -> Result<Vec<arbor_core::process::ProcessInfo>, DaemonClientError>;
    fn start_process(
        &self,
        name: &str,
    ) -> Result<arbor_core::process::ProcessInfo, DaemonClientError>;
    fn stop_process(
        &self,
        name: &str,
    ) -> Result<arbor_core::process::ProcessInfo, DaemonClientError>;
    fn restart_process(
        &self,
        name: &str,
    ) -> Result<arbor_core::process::ProcessInfo, DaemonClientError>;
    fn list_tasks(&self) -> Result<Vec<TaskInfo>, DaemonClientError>;
    fn run_task(&self, name: &str) -> Result<TaskInfo, DaemonClientError>;
    fn task_history(&self, name: &str) -> Result<Vec<TaskExecution>, DaemonClientError>;
}

impl DaemonApi for DaemonClient {
    fn health(&self) -> Result<HealthResponse, DaemonClientError> {
        self.health()
    }

    fn list_repositories(&self) -> Result<Vec<RepositoryDto>, DaemonClientError> {
        self.list_repositories()
    }

    fn list_worktrees(
        &self,
        repo_root: Option<&str>,
    ) -> Result<Vec<WorktreeDto>, DaemonClientError> {
        self.list_worktrees(repo_root)
    }

    fn create_worktree(
        &self,
        request: &CreateWorktreeRequest,
    ) -> Result<WorktreeMutationResponse, DaemonClientError> {
        self.create_worktree(request)
    }

    fn delete_worktree(
        &self,
        request: &DeleteWorktreeRequest,
    ) -> Result<WorktreeMutationResponse, DaemonClientError> {
        self.delete_worktree(request)
    }

    fn list_changed_files(&self, path: &str) -> Result<Vec<ChangedFileDto>, DaemonClientError> {
        self.list_changed_files(path)
    }

    fn commit_worktree(
        &self,
        request: &CommitWorktreeRequest,
    ) -> Result<GitActionResponse, DaemonClientError> {
        self.commit_worktree(request)
    }

    fn push_worktree(
        &self,
        request: &PushWorktreeRequest,
    ) -> Result<GitActionResponse, DaemonClientError> {
        self.push_worktree(request)
    }

    fn list_terminals(
        &self,
    ) -> Result<Vec<arbor_core::daemon::DaemonSessionRecord>, DaemonClientError> {
        self.list_terminals()
    }

    fn create_terminal(
        &self,
        request: &CreateTerminalRequest,
    ) -> Result<CreateTerminalResponse, DaemonClientError> {
        self.create_terminal(request)
    }

    fn read_terminal_output(
        &self,
        session_id: &str,
        max_lines: Option<usize>,
    ) -> Result<arbor_core::daemon::TerminalSnapshot, DaemonClientError> {
        self.read_terminal_output(session_id, max_lines)
    }

    fn write_terminal_input(&self, session_id: &str, data: &[u8]) -> Result<(), DaemonClientError> {
        self.write_terminal_input(session_id, data)
    }

    fn resize_terminal(
        &self,
        session_id: &str,
        request: &TerminalResizeRequest,
    ) -> Result<(), DaemonClientError> {
        self.resize_terminal(session_id, request)
    }

    fn signal_terminal(
        &self,
        session_id: &str,
        request: &TerminalSignalRequest,
    ) -> Result<(), DaemonClientError> {
        self.signal_terminal(session_id, request)
    }

    fn detach_terminal(&self, session_id: &str) -> Result<(), DaemonClientError> {
        self.detach_terminal(session_id)
    }

    fn kill_terminal(&self, session_id: &str) -> Result<(), DaemonClientError> {
        self.kill_terminal(session_id)
    }

    fn list_agent_activity(&self) -> Result<Vec<AgentSessionDto>, DaemonClientError> {
        self.list_agent_activity()
    }

    fn list_processes(&self) -> Result<Vec<arbor_core::process::ProcessInfo>, DaemonClientError> {
        self.list_processes()
    }

    fn start_all_processes(
        &self,
    ) -> Result<Vec<arbor_core::process::ProcessInfo>, DaemonClientError> {
        self.start_all_processes()
    }

    fn stop_all_processes(
        &self,
    ) -> Result<Vec<arbor_core::process::ProcessInfo>, DaemonClientError> {
        self.stop_all_processes()
    }

    fn start_process(
        &self,
        name: &str,
    ) -> Result<arbor_core::process::ProcessInfo, DaemonClientError> {
        self.start_process(name)
    }

    fn stop_process(
        &self,
        name: &str,
    ) -> Result<arbor_core::process::ProcessInfo, DaemonClientError> {
        self.stop_process(name)
    }

    fn restart_process(
        &self,
        name: &str,
    ) -> Result<arbor_core::process::ProcessInfo, DaemonClientError> {
        self.restart_process(name)
    }

    fn list_tasks(&self) -> Result<Vec<TaskInfo>, DaemonClientError> {
        self.list_tasks()
    }

    fn run_task(&self, name: &str) -> Result<TaskInfo, DaemonClientError> {
        self.run_task(name)
    }

    fn task_history(&self, name: &str) -> Result<Vec<TaskExecution>, DaemonClientError> {
        self.task_history(name)
    }
}

#[derive(Clone)]
pub struct ArborMcp {
    daemon: Arc<dyn DaemonApi>,
    tool_router: ToolRouter<Self>,
}

impl Default for ArborMcp {
    fn default() -> Self {
        Self::new()
    }
}

impl ArborMcp {
    pub fn new() -> Self {
        Self::with_client(Arc::new(DaemonClient::from_env()))
    }

    pub fn with_client(daemon: Arc<dyn DaemonApi>) -> Self {
        Self {
            daemon,
            tool_router: Self::create_tool_router(),
        }
    }
}

#[tool_handler]
impl ServerHandler for ArborMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_prompts()
            .build();
        info.server_info = Implementation::from_build_env();
        info.instructions = Some(
            "Arbor MCP server. Tools, prompts, and resources are backed by arbor-httpd. Configure ARBOR_DAEMON_URL for a non-default daemon address and ARBOR_DAEMON_AUTH_TOKEN for remote authenticated daemons."
                .to_owned(),
        );
        info
    }

    fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        std::future::ready(self.list_resources_result(request))
    }

    fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, ErrorData>> + Send + '_ {
        std::future::ready(self.list_resource_templates_result(request))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, ErrorData>> + Send + '_ {
        std::future::ready(self.read_resource_contents(&request.uri))
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, ErrorData>> + Send + '_ {
        std::future::ready({
            let result = ListPromptsResult {
                prompts: self.prompt_definitions(),
                ..Default::default()
            };
            Ok(result)
        })
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResult, ErrorData>> + Send + '_ {
        std::future::ready(self.prompt_response(request))
    }
}

fn string_error(error: DaemonClientError) -> String {
    error.to_string()
}

fn map_daemon_error(error: DaemonClientError) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}

#[cfg(feature = "stdio-server")]
pub async fn serve_stdio() -> anyhow::Result<()> {
    let service = ArborMcp::new().serve(rmcp::transport::io::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        arbor_core::{
            daemon::{DaemonSessionRecord, TerminalSessionState, TerminalSnapshot},
            process::{ProcessInfo, ProcessSource, ProcessStatus},
        },
    };

    #[derive(Default)]
    struct FakeDaemon;

    impl DaemonApi for FakeDaemon {
        fn health(&self) -> Result<HealthResponse, DaemonClientError> {
            Ok(HealthResponse {
                status: "ok".to_owned(),
                version: "test".to_owned(),
            })
        }

        fn list_repositories(&self) -> Result<Vec<RepositoryDto>, DaemonClientError> {
            Ok(vec![RepositoryDto {
                root: "/tmp/repo".to_owned(),
                label: "repo".to_owned(),
                github_repo_slug: None,
                avatar_url: None,
            }])
        }

        fn list_worktrees(
            &self,
            _repo_root: Option<&str>,
        ) -> Result<Vec<WorktreeDto>, DaemonClientError> {
            Ok(vec![WorktreeDto {
                repo_root: "/tmp/repo".to_owned(),
                path: "/tmp/repo".to_owned(),
                branch: "main".to_owned(),
                is_primary_checkout: true,
                last_activity_unix_ms: None,
                diff_additions: None,
                diff_deletions: None,
                pr_number: None,
                pr_url: None,
                processes: vec![],
            }])
        }

        fn create_worktree(
            &self,
            request: &CreateWorktreeRequest,
        ) -> Result<WorktreeMutationResponse, DaemonClientError> {
            Ok(WorktreeMutationResponse {
                repo_root: request.repo_root.clone(),
                path: request.path.clone(),
                branch: request.branch.clone(),
                deleted_branch: None,
                message: "created".to_owned(),
            })
        }

        fn delete_worktree(
            &self,
            request: &DeleteWorktreeRequest,
        ) -> Result<WorktreeMutationResponse, DaemonClientError> {
            Ok(WorktreeMutationResponse {
                repo_root: request.repo_root.clone(),
                path: request.path.clone(),
                branch: Some("feature".to_owned()),
                deleted_branch: Some("feature".to_owned()),
                message: "deleted".to_owned(),
            })
        }

        fn list_changed_files(
            &self,
            _path: &str,
        ) -> Result<Vec<ChangedFileDto>, DaemonClientError> {
            Ok(vec![ChangedFileDto {
                path: "src/main.rs".to_owned(),
                kind: "modified".to_owned(),
                additions: 3,
                deletions: 1,
            }])
        }

        fn commit_worktree(
            &self,
            request: &CommitWorktreeRequest,
        ) -> Result<GitActionResponse, DaemonClientError> {
            Ok(GitActionResponse {
                path: request.path.clone(),
                branch: Some("main".to_owned()),
                message: "commit complete".to_owned(),
                commit_message: request
                    .message
                    .clone()
                    .or_else(|| Some("generated".to_owned())),
            })
        }

        fn push_worktree(
            &self,
            request: &PushWorktreeRequest,
        ) -> Result<GitActionResponse, DaemonClientError> {
            Ok(GitActionResponse {
                path: request.path.clone(),
                branch: Some("main".to_owned()),
                message: "push complete".to_owned(),
                commit_message: None,
            })
        }

        fn list_terminals(&self) -> Result<Vec<DaemonSessionRecord>, DaemonClientError> {
            Ok(vec![DaemonSessionRecord {
                session_id: "daemon-1".into(),
                workspace_id: "/tmp/repo".into(),
                cwd: "/tmp/repo".into(),
                shell: "/bin/zsh".to_owned(),
                cols: 120,
                rows: 35,
                title: Some("shell".to_owned()),
                last_command: None,
                output_tail: None,
                exit_code: None,
                state: Some(TerminalSessionState::Running),
                updated_at_unix_ms: None,
                root_pid: None,
            }])
        }

        fn create_terminal(
            &self,
            request: &CreateTerminalRequest,
        ) -> Result<CreateTerminalResponse, DaemonClientError> {
            Ok(CreateTerminalResponse {
                is_new_session: true,
                session: DaemonSessionRecord {
                    session_id: request
                        .session_id
                        .clone()
                        .unwrap_or_else(|| "daemon-1".into()),
                    workspace_id: request
                        .workspace_id
                        .clone()
                        .unwrap_or_else(|| request.cwd.clone().into()),
                    cwd: request.cwd.clone().into(),
                    shell: request
                        .shell
                        .clone()
                        .unwrap_or_else(|| "/bin/zsh".to_owned()),
                    cols: request.cols.unwrap_or(120),
                    rows: request.rows.unwrap_or(35),
                    title: request.title.clone(),
                    last_command: None,
                    output_tail: None,
                    exit_code: None,
                    state: Some(TerminalSessionState::Running),
                    updated_at_unix_ms: None,
                    root_pid: None,
                },
            })
        }

        fn read_terminal_output(
            &self,
            session_id: &str,
            _max_lines: Option<usize>,
        ) -> Result<TerminalSnapshot, DaemonClientError> {
            Ok(TerminalSnapshot {
                session_id: session_id.into(),
                output_tail: "ok".to_owned(),
                styled_lines: vec![],
                cursor: None,
                modes: Default::default(),
                exit_code: None,
                state: TerminalSessionState::Running,
                updated_at_unix_ms: None,
            })
        }

        fn write_terminal_input(
            &self,
            _session_id: &str,
            _data: &[u8],
        ) -> Result<(), DaemonClientError> {
            Ok(())
        }

        fn resize_terminal(
            &self,
            _session_id: &str,
            _request: &TerminalResizeRequest,
        ) -> Result<(), DaemonClientError> {
            Ok(())
        }

        fn signal_terminal(
            &self,
            _session_id: &str,
            _request: &TerminalSignalRequest,
        ) -> Result<(), DaemonClientError> {
            Ok(())
        }

        fn detach_terminal(&self, _session_id: &str) -> Result<(), DaemonClientError> {
            Ok(())
        }

        fn kill_terminal(&self, _session_id: &str) -> Result<(), DaemonClientError> {
            Ok(())
        }

        fn list_agent_activity(&self) -> Result<Vec<AgentSessionDto>, DaemonClientError> {
            Ok(vec![AgentSessionDto {
                cwd: "/tmp/repo".to_owned(),
                state: "working".to_owned(),
                updated_at_unix_ms: 1,
            }])
        }

        fn list_processes(&self) -> Result<Vec<ProcessInfo>, DaemonClientError> {
            Ok(vec![ProcessInfo {
                id: "procfile:/tmp/repo:web".to_owned(),
                name: "web".to_owned(),
                command: "cargo run".to_owned(),
                repo_root: "/tmp/repo".to_owned(),
                workspace_id: "/tmp/repo".to_owned(),
                source: ProcessSource::Procfile,
                status: ProcessStatus::Running,
                exit_code: None,
                restart_count: 0,
                memory_bytes: Some(134_217_728),
                session_id: Some("process-web".to_owned()),
            }])
        }

        fn start_all_processes(&self) -> Result<Vec<ProcessInfo>, DaemonClientError> {
            self.list_processes()
        }

        fn stop_all_processes(&self) -> Result<Vec<ProcessInfo>, DaemonClientError> {
            self.list_processes()
        }

        fn start_process(&self, _name: &str) -> Result<ProcessInfo, DaemonClientError> {
            Ok(self
                .list_processes()?
                .into_iter()
                .next()
                .unwrap_or(ProcessInfo {
                    id: "procfile:/tmp/repo:web".to_owned(),
                    name: "web".to_owned(),
                    command: "cargo run".to_owned(),
                    repo_root: "/tmp/repo".to_owned(),
                    workspace_id: "/tmp/repo".to_owned(),
                    source: ProcessSource::Procfile,
                    status: ProcessStatus::Running,
                    exit_code: None,
                    restart_count: 0,
                    memory_bytes: Some(134_217_728),
                    session_id: Some("process-web".to_owned()),
                }))
        }

        fn stop_process(&self, _name: &str) -> Result<ProcessInfo, DaemonClientError> {
            self.start_process(_name)
        }

        fn restart_process(&self, _name: &str) -> Result<ProcessInfo, DaemonClientError> {
            self.start_process(_name)
        }

        fn list_tasks(&self) -> Result<Vec<TaskInfo>, DaemonClientError> {
            Ok(vec![TaskInfo {
                name: "check-issues".to_owned(),
                schedule: "*/15 * * * *".to_owned(),
                command: "./check.sh".to_owned(),
                status: arbor_core::task::TaskStatus::Idle,
                has_trigger: true,
                last_run_unix_ms: None,
                last_exit_code: None,
                next_run_unix_ms: None,
                run_count: 0,
            }])
        }

        fn run_task(&self, _name: &str) -> Result<TaskInfo, DaemonClientError> {
            Ok(self.list_tasks()?.into_iter().next().unwrap_or(TaskInfo {
                name: "check-issues".to_owned(),
                schedule: "*/15 * * * *".to_owned(),
                command: "./check.sh".to_owned(),
                status: arbor_core::task::TaskStatus::Running,
                has_trigger: true,
                last_run_unix_ms: None,
                last_exit_code: None,
                next_run_unix_ms: None,
                run_count: 0,
            }))
        }

        fn task_history(&self, _name: &str) -> Result<Vec<TaskExecution>, DaemonClientError> {
            Ok(vec![])
        }
    }

    #[test]
    fn advertises_tools_prompts_and_resources() {
        let server = ArborMcp::with_client(Arc::new(FakeDaemon));
        let info = server.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
        assert!(info.capabilities.prompts.is_some());
    }

    #[test]
    fn prompt_catalog_is_populated() {
        let server = ArborMcp::with_client(Arc::new(FakeDaemon));
        let prompts = server.prompt_definitions();
        assert_eq!(prompts.len(), 3);
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt.name == "review-worktree")
        );
    }

    #[test]
    fn reads_health_resource() {
        let server = ArborMcp::with_client(Arc::new(FakeDaemon));
        let result = server
            .read_resource_contents("arbor://health")
            .unwrap_or_else(|e| panic!("health resource should be readable: {e:?}"));
        assert_eq!(result.contents.len(), 1);
    }

    #[test]
    fn tool_catalog_contains_structured_tools() {
        let server = ArborMcp::with_client(Arc::new(FakeDaemon));
        let tools = server.tool_router.list_all();
        assert!(tools.iter().any(|tool| tool.name == "list_repositories"));
        assert!(
            tools
                .iter()
                .find(|tool| tool.name == "list_repositories")
                .and_then(|tool| tool.output_schema.as_ref())
                .is_some()
        );
    }
}
