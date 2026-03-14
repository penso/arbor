use {
    graphql_client::GraphQLQuery,
    serde::Deserialize,
    std::{
        process::{Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Success,
    Failure,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeableState {
    Conflicting,
    Mergeable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStateStatus {
    Behind,
    Blocked,
    Clean,
    Dirty,
    Draft,
    HasHooks,
    Unknown,
    Unstable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrDetails {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: PrState,
    pub additions: usize,
    pub deletions: usize,
    pub review_decision: ReviewDecision,
    pub mergeable: MergeableState,
    pub merge_state_status: MergeStateStatus,
    pub passed_checks: usize,
    pub checks_status: CheckStatus,
    pub checks: Vec<(String, CheckStatus)>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReviewPullRequest {
    pub(crate) number: u64,
    pub(crate) title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhReviewPrResponse {
    number: u64,
    title: String,
}

#[derive(Deserialize)]
struct GhCheckContext {
    name: Option<String>,
    context: Option<String>,
    conclusion: Option<String>,
    status: Option<String>,
    state: Option<String>,
}

impl GhCheckContext {
    fn display_name(&self) -> String {
        self.name
            .as_deref()
            .or(self.context.as_deref())
            .unwrap_or("check")
            .to_owned()
    }

    fn to_check_status(&self) -> CheckStatus {
        if self
            .status
            .as_deref()
            .is_some_and(|status| !status.eq_ignore_ascii_case("completed"))
        {
            return CheckStatus::Pending;
        }

        if let Some(conclusion) = &self.conclusion {
            return match conclusion.as_str() {
                "" => CheckStatus::Pending,
                "SUCCESS" | "success" | "NEUTRAL" | "neutral" | "SKIPPED" | "skipped" => {
                    CheckStatus::Success
                },
                _ => CheckStatus::Failure,
            };
        }
        if let Some(state) = &self.state {
            return match state.as_str() {
                "SUCCESS" | "success" => CheckStatus::Success,
                "FAILURE" | "failure" | "ERROR" | "error" => CheckStatus::Failure,
                _ => CheckStatus::Pending,
            };
        }
        if let Some(status) = &self.status {
            return match status.as_str() {
                "COMPLETED" | "completed" => CheckStatus::Success,
                _ => CheckStatus::Pending,
            };
        }
        CheckStatus::Pending
    }
}

fn sort_checks_for_display(checks: &mut [(String, CheckStatus)]) {
    checks.sort_by(|left, right| {
        check_status_sort_key(left.1)
            .cmp(&check_status_sort_key(right.1))
            .then(left.0.cmp(&right.0))
    });
}

fn check_status_sort_key(status: CheckStatus) -> usize {
    match status {
        CheckStatus::Failure => 0,
        CheckStatus::Pending => 1,
        CheckStatus::Success => 2,
    }
}

const GITHUB_GRAPHQL_API_URL: &str = "https://api.github.com/graphql";
static GITHUB_GRAPHQL_REQUEST_COUNT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize)]
struct GitHubGraphqlResponse<T> {
    data: Option<T>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    errors: Vec<GitHubGraphqlError>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubGraphqlError {
    message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestDetailsGraphqlData {
    repository: Option<PullRequestDetailsGraphqlRepository>,
    rate_limit: Option<PullRequestDetailsGraphqlRateLimit>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestDetailsGraphqlRepository {
    pull_requests: PullRequestDetailsGraphqlPullRequestConnection,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestDetailsGraphqlPullRequestConnection {
    #[serde(default)]
    nodes: Vec<Option<PullRequestDetailsGraphqlPullRequest>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestDetailsGraphqlPullRequest {
    number: u64,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
    additions: usize,
    deletions: usize,
    review_decision: Option<String>,
    mergeable: Option<String>,
    merge_state_status: Option<String>,
    commits: PullRequestDetailsGraphqlCommitConnection,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestDetailsGraphqlCommitConnection {
    #[serde(default)]
    nodes: Vec<Option<PullRequestDetailsGraphqlCommitNode>>,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestDetailsGraphqlCommitNode {
    commit: PullRequestDetailsGraphqlCommit,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestDetailsGraphqlCommit {
    status_check_rollup: Option<PullRequestDetailsGraphqlStatusCheckRollup>,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestDetailsGraphqlStatusCheckRollup {
    contexts: PullRequestDetailsGraphqlStatusContextConnection,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestDetailsGraphqlStatusContextConnection {
    #[serde(default)]
    nodes: Vec<Option<PullRequestDetailsGraphqlStatusContextNode>>,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestDetailsGraphqlStatusContextNode {
    #[serde(rename = "__typename")]
    type_name: String,
    name: Option<String>,
    conclusion: Option<String>,
    status: Option<String>,
    context: Option<String>,
    state: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestDetailsGraphqlRateLimit {
    cost: u64,
    limit: u64,
    remaining: u64,
    reset_at: String,
    used: u64,
}

#[derive(Debug, Default)]
struct GitHubRateLimitHeaders {
    limit: Option<u64>,
    remaining: Option<u64>,
    used: Option<u64>,
    reset_epoch_seconds: Option<u64>,
    retry_after_seconds: Option<u64>,
    resource: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PullRequestDetailsOutcome {
    pub(crate) details: Option<PrDetails>,
    pub(crate) rate_limited_until: Option<SystemTime>,
}

pub fn pull_request_details(
    repo_slug: &str,
    branch: &str,
    github_token: Option<&str>,
) -> PullRequestDetailsOutcome {
    let Some((owner, repo_name)) = repo_slug.split_once('/') else {
        return PullRequestDetailsOutcome::default();
    };
    let Some(token) = crate::resolve_github_access_token(github_token) else {
        return PullRequestDetailsOutcome::default();
    };
    let request_count = GITHUB_GRAPHQL_REQUEST_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let request_body = crate::graphql::PullRequestDetails::build_query(
        crate::graphql::pull_request_details::Variables {
            owner: owner.to_owned(),
            repo: repo_name.to_owned(),
            head: branch.to_owned(),
        },
    );
    let request_body_json = match serde_json::to_string(&request_body) {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(
                request_count,
                repo = %repo_slug,
                head = %branch,
                %error,
                "failed to serialize GitHub GraphQL PR details request"
            );
            return PullRequestDetailsOutcome::default();
        },
    };

    let response = github_graphql_http_agent()
        .post(GITHUB_GRAPHQL_API_URL)
        .header("Accept", "application/json")
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", "Arbor")
        .send(&request_body_json);

    let mut response = match response {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                request_count,
                repo = %repo_slug,
                head = %branch,
                %error,
                "GitHub GraphQL PR details request failed before a response arrived"
            );
            return PullRequestDetailsOutcome::default();
        },
    };

    let status = response.status();
    let status_code = status.as_u16();
    let rate_limit_headers = parse_rate_limit_headers(response.headers());
    let response_body_json = match response.body_mut().read_to_string() {
        Ok(body) => body,
        Err(error) => {
            let rate_limited_until = detect_rate_limited_until(
                status_code,
                &rate_limit_headers,
                None,
                &[],
                SystemTime::now(),
            );
            tracing::warn!(
                request_count,
                repo = %repo_slug,
                head = %branch,
                http_status = %status,
                %error,
                header_rate_limit_remaining = rate_limit_headers.remaining,
                header_rate_limit_limit = rate_limit_headers.limit,
                header_rate_limit_used = rate_limit_headers.used,
                header_rate_limit_reset_epoch_seconds = rate_limit_headers.reset_epoch_seconds,
                header_rate_limit_resource = rate_limit_headers.resource.as_deref(),
                "GitHub GraphQL PR details response was not valid JSON"
            );
            return PullRequestDetailsOutcome {
                details: None,
                rate_limited_until,
            };
        },
    };
    let response_body: GitHubGraphqlResponse<PullRequestDetailsGraphqlData> =
        match serde_json::from_str(&response_body_json) {
            Ok(body) => body,
            Err(error) => {
                let rate_limited_until = detect_rate_limited_until(
                    status_code,
                    &rate_limit_headers,
                    None,
                    &[],
                    SystemTime::now(),
                );
                tracing::warn!(
                    request_count,
                    repo = %repo_slug,
                    head = %branch,
                    http_status = %status,
                    %error,
                    header_rate_limit_remaining = rate_limit_headers.remaining,
                    header_rate_limit_limit = rate_limit_headers.limit,
                    header_rate_limit_used = rate_limit_headers.used,
                    header_rate_limit_reset_epoch_seconds = rate_limit_headers.reset_epoch_seconds,
                    header_rate_limit_resource = rate_limit_headers.resource.as_deref(),
                    "GitHub GraphQL PR details response body could not be parsed"
                );
                return PullRequestDetailsOutcome {
                    details: None,
                    rate_limited_until,
                };
            },
        };

    let graphql_rate_limit = response_body
        .data
        .as_ref()
        .and_then(|data| data.rate_limit.as_ref());
    let error_messages = collect_graphql_error_messages(&response_body);
    let rate_limited_until = detect_rate_limited_until(
        status_code,
        &rate_limit_headers,
        graphql_rate_limit,
        &error_messages,
        SystemTime::now(),
    );
    if status != 200 || !error_messages.is_empty() {
        let combined_errors = (!error_messages.is_empty()).then(|| error_messages.join(" | "));
        tracing::warn!(
            request_count,
            repo = %repo_slug,
            head = %branch,
            http_status = %status,
            errors = combined_errors.as_deref(),
            header_rate_limit_remaining = rate_limit_headers.remaining,
            header_rate_limit_limit = rate_limit_headers.limit,
            header_rate_limit_used = rate_limit_headers.used,
            header_rate_limit_reset_epoch_seconds = rate_limit_headers.reset_epoch_seconds,
            header_rate_limit_resource = rate_limit_headers.resource.as_deref(),
            graphql_rate_limit_cost = graphql_rate_limit.map(|value| value.cost),
            graphql_rate_limit_remaining = graphql_rate_limit.map(|value| value.remaining),
            graphql_rate_limit_limit = graphql_rate_limit.map(|value| value.limit),
            graphql_rate_limit_used = graphql_rate_limit.map(|value| value.used),
            graphql_rate_limit_reset_at = graphql_rate_limit.map(|value| value.reset_at.as_str()),
            rate_limited_until_epoch_seconds = rate_limited_until.and_then(system_time_epoch_seconds),
            "GitHub GraphQL PR details request returned an error"
        );
    }

    let details = response_body
        .data
        .as_ref()
        .and_then(|data| data.repository.as_ref())
        .and_then(|repository| repository.pull_requests.nodes.iter().flatten().next())
        .cloned()
        .map(parse_graphql_pr_details);

    if let Some(details) = details.as_ref() {
        let data = response_body.data.as_ref();
        tracing::info!(
            request_count,
            repo = %repo_slug,
            head = %branch,
            http_status = %status,
            pull_request_number = details.number,
            header_rate_limit_remaining = rate_limit_headers.remaining,
            header_rate_limit_limit = rate_limit_headers.limit,
            header_rate_limit_used = rate_limit_headers.used,
            header_rate_limit_reset_epoch_seconds = rate_limit_headers.reset_epoch_seconds,
            header_rate_limit_resource = rate_limit_headers.resource.as_deref(),
            graphql_rate_limit_cost = data.and_then(|value| value.rate_limit.as_ref()).map(|value| value.cost),
            graphql_rate_limit_remaining = data.and_then(|value| value.rate_limit.as_ref()).map(|value| value.remaining),
            graphql_rate_limit_limit = data.and_then(|value| value.rate_limit.as_ref()).map(|value| value.limit),
            graphql_rate_limit_used = data.and_then(|value| value.rate_limit.as_ref()).map(|value| value.used),
            graphql_rate_limit_reset_at = data.and_then(|value| value.rate_limit.as_ref()).map(|value| value.reset_at.as_str()),
            rate_limited_until_epoch_seconds = rate_limited_until.and_then(system_time_epoch_seconds),
            "GitHub GraphQL PR details request completed"
        );
    }

    PullRequestDetailsOutcome {
        details,
        rate_limited_until,
    }
}

fn github_graphql_http_agent() -> ureq::Agent {
    let config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .build();
    ureq::Agent::new_with_config(config)
}

fn collect_graphql_error_messages<T>(response_body: &GitHubGraphqlResponse<T>) -> Vec<&str> {
    let mut error_messages = Vec::new();
    if let Some(message) = response_body.message.as_deref() {
        error_messages.push(message);
    }
    for error in &response_body.errors {
        let message = error.message.as_str();
        if !error_messages.contains(&message) {
            error_messages.push(message);
        }
    }
    error_messages
}

fn parse_rate_limit_headers(headers: &ureq::http::HeaderMap) -> GitHubRateLimitHeaders {
    GitHubRateLimitHeaders {
        limit: parse_u64_header(headers, "x-ratelimit-limit"),
        remaining: parse_u64_header(headers, "x-ratelimit-remaining"),
        used: parse_u64_header(headers, "x-ratelimit-used"),
        reset_epoch_seconds: parse_u64_header(headers, "x-ratelimit-reset"),
        retry_after_seconds: parse_u64_header(headers, "retry-after"),
        resource: headers
            .get("x-ratelimit-resource")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    }
}

fn parse_u64_header(headers: &ureq::http::HeaderMap, key: &str) -> Option<u64> {
    headers
        .get(key)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn detect_rate_limited_until(
    status_code: u16,
    rate_limit_headers: &GitHubRateLimitHeaders,
    graphql_rate_limit: Option<&PullRequestDetailsGraphqlRateLimit>,
    error_messages: &[&str],
    now: SystemTime,
) -> Option<SystemTime> {
    let reset_until = || {
        rate_limit_headers
            .reset_epoch_seconds
            .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds))
            .filter(|until| *until > now)
    };
    let lower_error_messages = error_messages
        .iter()
        .map(|message| message.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mentions_rate_limit = lower_error_messages
        .iter()
        .any(|message| message.contains("rate limit"));
    let mentions_secondary_limit = lower_error_messages
        .iter()
        .any(|message| message.contains("secondary rate limit"));

    if let Some(retry_after_seconds) = rate_limit_headers.retry_after_seconds
        && (status_code == 403 || status_code == 429 || mentions_rate_limit)
    {
        return Some(now + Duration::from_secs(retry_after_seconds));
    }

    if rate_limit_headers.remaining == Some(0)
        || graphql_rate_limit.is_some_and(|value| value.remaining == 0)
    {
        return reset_until();
    }

    if mentions_secondary_limit {
        return Some(now + Duration::from_secs(60));
    }

    if mentions_rate_limit {
        return reset_until();
    }

    None
}

fn system_time_epoch_seconds(system_time: SystemTime) -> Option<u64> {
    system_time
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn parse_graphql_pr_details(response: PullRequestDetailsGraphqlPullRequest) -> PrDetails {
    let state = if response.is_draft {
        PrState::Draft
    } else {
        match response.state.as_str() {
            "MERGED" => PrState::Merged,
            "CLOSED" => PrState::Closed,
            _ => PrState::Open,
        }
    };

    let review_decision = match response.review_decision.as_deref() {
        Some("APPROVED") => ReviewDecision::Approved,
        Some("CHANGES_REQUESTED") => ReviewDecision::ChangesRequested,
        _ => ReviewDecision::Pending,
    };
    let mergeable = match response.mergeable.as_deref() {
        Some("CONFLICTING") => MergeableState::Conflicting,
        Some("MERGEABLE") => MergeableState::Mergeable,
        _ => MergeableState::Unknown,
    };
    let merge_state_status = match response.merge_state_status.as_deref() {
        Some("BEHIND") => MergeStateStatus::Behind,
        Some("BLOCKED") => MergeStateStatus::Blocked,
        Some("CLEAN") => MergeStateStatus::Clean,
        Some("DIRTY") => MergeStateStatus::Dirty,
        Some("DRAFT") => MergeStateStatus::Draft,
        Some("HAS_HOOKS") => MergeStateStatus::HasHooks,
        Some("UNSTABLE") => MergeStateStatus::Unstable,
        _ => MergeStateStatus::Unknown,
    };

    let mut checks = response
        .commits
        .nodes
        .into_iter()
        .flatten()
        .next()
        .and_then(|commit| commit.commit.status_check_rollup)
        .map(|rollup| {
            rollup
                .contexts
                .nodes
                .into_iter()
                .flatten()
                .filter_map(graphql_status_context_to_check)
                .map(|context| (context.display_name(), context.to_check_status()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let passed_checks = checks
        .iter()
        .filter(|(_, status)| *status == CheckStatus::Success)
        .count();

    let checks_status = if checks.is_empty() {
        CheckStatus::Pending
    } else if checks
        .iter()
        .any(|(_, status)| *status == CheckStatus::Failure)
    {
        CheckStatus::Failure
    } else if checks
        .iter()
        .all(|(_, status)| *status == CheckStatus::Success)
    {
        CheckStatus::Success
    } else {
        CheckStatus::Pending
    };
    sort_checks_for_display(&mut checks);

    PrDetails {
        number: response.number,
        title: response.title,
        url: response.url,
        state,
        additions: response.additions,
        deletions: response.deletions,
        review_decision,
        mergeable,
        merge_state_status,
        passed_checks,
        checks_status,
        checks,
    }
}

fn graphql_status_context_to_check(
    node: PullRequestDetailsGraphqlStatusContextNode,
) -> Option<GhCheckContext> {
    match node.type_name.as_str() {
        "CheckRun" => Some(GhCheckContext {
            name: node.name,
            context: None,
            conclusion: node.conclusion,
            status: node.status,
            state: None,
        }),
        "StatusContext" => Some(GhCheckContext {
            name: None,
            context: node.context,
            conclusion: None,
            status: None,
            state: node.state,
        }),
        _ => None,
    }
}

pub trait GitHubService: Send + Sync {
    fn create_pull_request(
        &self,
        repo_slug: &str,
        title: &str,
        branch: &str,
        base_branch: &str,
        token: &str,
    ) -> Result<String, String>;

    fn pull_request_number(&self, repo_slug: &str, branch: &str, token: &str) -> Option<u64>;
}

pub struct OctocrabGitHubService;

impl GitHubService for OctocrabGitHubService {
    fn create_pull_request(
        &self,
        repo_slug: &str,
        title: &str,
        branch: &str,
        base_branch: &str,
        token: &str,
    ) -> Result<String, String> {
        let (owner, repo_name) = repo_slug
            .split_once('/')
            .ok_or_else(|| format!("invalid repository slug: {repo_slug}"))?;

        let owner = owner.to_owned();
        let repo_name = repo_name.to_owned();
        let title = title.to_owned();
        let branch = branch.to_owned();
        let base_branch = base_branch.to_owned();
        let token = token.to_owned();

        let runtime = tokio::runtime::Runtime::new()
            .map_err(|error| format!("failed to create runtime: {error}"))?;

        runtime.block_on(async move {
            let octocrab = octocrab::Octocrab::builder()
                .personal_token(token)
                .build()
                .map_err(|error| format!("failed to create GitHub client: {error}"))?;

            let pr = octocrab
                .pulls(&owner, &repo_name)
                .create(&title, &branch, &base_branch)
                .send()
                .await
                .map_err(|error| format!("failed to create pull request: {error}"))?;

            let url = pr.html_url.map(|u| u.to_string()).unwrap_or_default();
            Ok(format!("created PR: {url}"))
        })
    }

    fn pull_request_number(&self, repo_slug: &str, branch: &str, token: &str) -> Option<u64> {
        let (owner, repo_name) = repo_slug.split_once('/')?;
        let owner = owner.to_owned();
        let repo_name = repo_name.to_owned();
        let branch = branch.to_owned();
        let token = token.to_owned();

        let runtime = tokio::runtime::Runtime::new().ok()?;
        runtime.block_on(async move {
            let octocrab = octocrab::Octocrab::builder()
                .personal_token(token)
                .build()
                .ok()?;

            let page = octocrab
                .pulls(&owner, &repo_name)
                .list()
                .head(format!("{owner}:{branch}"))
                .state(octocrab::params::State::All)
                .per_page(1)
                .send()
                .await
                .ok()?;

            page.items.first().map(|pr| pr.number)
        })
    }
}

pub fn default_github_service() -> Arc<dyn GitHubService> {
    Arc::new(OctocrabGitHubService)
}

pub fn github_access_token_from_gh_cli() -> Option<String> {
    let output = Command::new("gh")
        .args(["auth", "token"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let token = stdout.trim();
    (!token.is_empty()).then_some(token.to_owned())
}

pub(crate) fn resolve_pull_request_for_review(
    repo_slug: &str,
    reference: &str,
    github_token: Option<&str>,
) -> Result<ReviewPullRequest, String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err("pull request reference is required".to_owned());
    }

    if let Some(pull_request) = resolve_pull_request_for_review_via_gh(repo_slug, reference) {
        return Ok(pull_request);
    }

    let pull_request_number = parse_pull_request_number(reference).ok_or_else(|| {
        "failed to resolve pull request with gh; use a PR number or GitHub pull request URL"
            .to_owned()
    })?;
    let token = crate::resolve_github_access_token(github_token).ok_or_else(|| {
        "failed to resolve pull request with gh and no GitHub token is available for API fallback"
            .to_owned()
    })?;

    resolve_pull_request_for_review_via_api(repo_slug, pull_request_number, &token)
}

fn resolve_pull_request_for_review_via_gh(
    repo_slug: &str,
    reference: &str,
) -> Option<ReviewPullRequest> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            "--repo",
            repo_slug,
            "--json",
            "number,title",
            reference,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let response: GhReviewPrResponse = serde_json::from_slice(&output.stdout).ok()?;
    Some(ReviewPullRequest {
        number: response.number,
        title: response.title,
    })
}

fn resolve_pull_request_for_review_via_api(
    repo_slug: &str,
    pull_request_number: u64,
    token: &str,
) -> Result<ReviewPullRequest, String> {
    let (owner, repo_name) = repo_slug
        .split_once('/')
        .ok_or_else(|| format!("invalid repository slug: {repo_slug}"))?;

    let owner = owner.to_owned();
    let repo_name = repo_name.to_owned();
    let token = token.to_owned();

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| format!("failed to create runtime: {error}"))?;

    runtime.block_on(async move {
        let octocrab = octocrab::Octocrab::builder()
            .personal_token(token)
            .build()
            .map_err(|error| format!("failed to create GitHub client: {error}"))?;

        let pull_request = octocrab
            .pulls(&owner, &repo_name)
            .get(pull_request_number)
            .await
            .map_err(|error| {
                format!("failed to resolve pull request #{pull_request_number}: {error}")
            })?;

        Ok(ReviewPullRequest {
            number: pull_request.number,
            title: pull_request
                .title
                .unwrap_or_else(|| format!("PR {}", pull_request.number)),
        })
    })
}

pub(crate) fn parse_pull_request_number(reference: &str) -> Option<u64> {
    let trimmed = reference.trim();
    if trimmed.is_empty() {
        return None;
    }

    let trimmed = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if let Ok(number) = trimmed.parse::<u64>() {
        return Some(number);
    }

    let url_path = trimmed.strip_prefix("https://github.com/")?;
    let segments = url_path.split('/').collect::<Vec<_>>();
    if segments.len() < 4 || segments[2] != "pull" {
        return None;
    }

    segments[3].parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use {
        super::{
            CheckStatus, GitHubGraphqlResponse, GitHubRateLimitHeaders, MergeStateStatus,
            MergeableState, PullRequestDetailsGraphqlCommit,
            PullRequestDetailsGraphqlCommitConnection, PullRequestDetailsGraphqlCommitNode,
            PullRequestDetailsGraphqlPullRequest, PullRequestDetailsGraphqlStatusCheckRollup,
            PullRequestDetailsGraphqlStatusContextConnection,
            PullRequestDetailsGraphqlStatusContextNode, collect_graphql_error_messages,
            detect_rate_limited_until, parse_graphql_pr_details, parse_pull_request_number,
        },
        std::time::{Duration, UNIX_EPOCH},
    };

    #[test]
    fn in_progress_checks_remain_pending_without_conclusion() {
        let details = parse_graphql_pr_details(PullRequestDetailsGraphqlPullRequest {
            number: 40,
            title: "Fix native toolbar buttons and linked worktree grouping".to_owned(),
            url: "https://github.com/penso/arbor/pull/40".to_owned(),
            state: "OPEN".to_owned(),
            is_draft: false,
            additions: 10,
            deletions: 2,
            review_decision: None,
            mergeable: Some("MERGEABLE".to_owned()),
            merge_state_status: Some("CLEAN".to_owned()),
            commits: PullRequestDetailsGraphqlCommitConnection {
                nodes: vec![Some(PullRequestDetailsGraphqlCommitNode {
                    commit: PullRequestDetailsGraphqlCommit {
                        status_check_rollup: Some(PullRequestDetailsGraphqlStatusCheckRollup {
                            contexts: PullRequestDetailsGraphqlStatusContextConnection {
                                nodes: vec![
                                    Some(PullRequestDetailsGraphqlStatusContextNode {
                                        type_name: "CheckRun".to_owned(),
                                        name: Some("Clippy".to_owned()),
                                        conclusion: Some(String::new()),
                                        status: Some("IN_PROGRESS".to_owned()),
                                        context: None,
                                        state: None,
                                    }),
                                    Some(PullRequestDetailsGraphqlStatusContextNode {
                                        type_name: "CheckRun".to_owned(),
                                        name: Some("Test".to_owned()),
                                        conclusion: Some(String::new()),
                                        status: Some("IN_PROGRESS".to_owned()),
                                        context: None,
                                        state: None,
                                    }),
                                ],
                            },
                        }),
                    },
                })],
            },
        });

        assert_eq!(details.checks_status, CheckStatus::Pending);
        assert_eq!(details.passed_checks, 0);
        assert_eq!(details.mergeable, MergeableState::Mergeable);
        assert_eq!(details.merge_state_status, MergeStateStatus::Clean);
        assert_eq!(details.checks, vec![
            ("Clippy".to_owned(), CheckStatus::Pending),
            ("Test".to_owned(), CheckStatus::Pending),
        ]);
    }

    #[test]
    fn parse_pull_request_number_supports_hash_number_and_url() {
        assert_eq!(parse_pull_request_number("#42"), Some(42));
        assert_eq!(parse_pull_request_number("42"), Some(42));
        assert_eq!(
            parse_pull_request_number("https://github.com/penso/arbor/pull/42"),
            Some(42)
        );
        assert_eq!(parse_pull_request_number("feature/test"), None);
    }

    #[test]
    fn graphql_pr_details_map_merge_state_and_checks() {
        let details = parse_graphql_pr_details(PullRequestDetailsGraphqlPullRequest {
            number: 42,
            title: "Tame the GraphQL goblin".to_owned(),
            url: "https://github.com/penso/arbor/pull/42".to_owned(),
            state: "OPEN".to_owned(),
            is_draft: false,
            additions: 120,
            deletions: 18,
            review_decision: Some("APPROVED".to_owned()),
            mergeable: Some("CONFLICTING".to_owned()),
            merge_state_status: Some("BEHIND".to_owned()),
            commits: PullRequestDetailsGraphqlCommitConnection {
                nodes: vec![Some(PullRequestDetailsGraphqlCommitNode {
                    commit: PullRequestDetailsGraphqlCommit {
                        status_check_rollup: Some(PullRequestDetailsGraphqlStatusCheckRollup {
                            contexts: PullRequestDetailsGraphqlStatusContextConnection {
                                nodes: vec![
                                    Some(PullRequestDetailsGraphqlStatusContextNode {
                                        type_name: "CheckRun".to_owned(),
                                        name: Some("Clippy".to_owned()),
                                        conclusion: Some("SUCCESS".to_owned()),
                                        status: Some("COMPLETED".to_owned()),
                                        context: None,
                                        state: None,
                                    }),
                                    Some(PullRequestDetailsGraphqlStatusContextNode {
                                        type_name: "StatusContext".to_owned(),
                                        name: None,
                                        conclusion: None,
                                        status: None,
                                        context: Some("ci/test".to_owned()),
                                        state: Some("PENDING".to_owned()),
                                    }),
                                ],
                            },
                        }),
                    },
                })],
            },
        });

        assert_eq!(details.number, 42);
        assert_eq!(details.review_decision, super::ReviewDecision::Approved);
        assert_eq!(details.mergeable, MergeableState::Conflicting);
        assert_eq!(details.merge_state_status, MergeStateStatus::Behind);
        assert_eq!(details.passed_checks, 1);
        assert_eq!(details.checks_status, CheckStatus::Pending);
        assert_eq!(details.checks, vec![
            ("ci/test".to_owned(), CheckStatus::Pending),
            ("Clippy".to_owned(), CheckStatus::Success),
        ]);
    }

    #[test]
    fn primary_rate_limit_uses_reset_epoch_header() {
        let now = UNIX_EPOCH + Duration::from_secs(50);
        let until = detect_rate_limited_until(
            200,
            &GitHubRateLimitHeaders {
                remaining: Some(0),
                reset_epoch_seconds: Some(120),
                ..GitHubRateLimitHeaders::default()
            },
            None,
            &[],
            now,
        );

        assert_eq!(until, Some(UNIX_EPOCH + Duration::from_secs(120)));
    }

    #[test]
    fn secondary_rate_limit_uses_retry_after_header() {
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let until = detect_rate_limited_until(
            403,
            &GitHubRateLimitHeaders {
                retry_after_seconds: Some(45),
                ..GitHubRateLimitHeaders::default()
            },
            None,
            &["You have exceeded a secondary rate limit"],
            now,
        );

        assert_eq!(until, Some(now + Duration::from_secs(45)));
    }

    #[test]
    fn secondary_rate_limit_defaults_to_one_minute_without_retry_after() {
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let until = detect_rate_limited_until(
            403,
            &GitHubRateLimitHeaders::default(),
            None,
            &["You have exceeded a secondary rate limit"],
            now,
        );

        assert_eq!(until, Some(now + Duration::from_secs(60)));
    }

    #[test]
    fn top_level_graphql_message_triggers_rate_limit_detection() {
        let response: GitHubGraphqlResponse<serde_json::Value> =
            serde_json::from_str(r#"{"message":"You have exceeded a secondary rate limit"}"#)
                .unwrap_or_else(|error| panic!("failed to deserialize response: {error}"));
        let error_messages = collect_graphql_error_messages(&response);

        let now = UNIX_EPOCH + Duration::from_secs(100);
        let until = detect_rate_limited_until(
            403,
            &GitHubRateLimitHeaders::default(),
            None,
            &error_messages,
            now,
        );

        assert_eq!(until, Some(now + Duration::from_secs(60)));
    }
}
