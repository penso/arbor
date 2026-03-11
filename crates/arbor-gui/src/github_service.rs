use {
    serde::Deserialize,
    std::{
        process::{Command, Stdio},
        sync::Arc,
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

#[derive(Debug, Clone)]
pub struct PrDetails {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: PrState,
    pub additions: usize,
    pub deletions: usize,
    pub review_decision: ReviewDecision,
    pub checks_status: CheckStatus,
    pub checks: Vec<(String, CheckStatus)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubUserProfile {
    pub login: String,
    pub avatar_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPrResponse {
    number: u64,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
    additions: usize,
    deletions: usize,
    review_decision: Option<String>,
    #[serde(default)]
    status_check_rollup: Vec<GhCheckContext>,
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

fn parse_pr_details(response: GhPrResponse) -> PrDetails {
    let state = if response.is_draft {
        PrState::Draft
    } else {
        match response.state.as_str() {
            "MERGED" | "merged" => PrState::Merged,
            "CLOSED" | "closed" => PrState::Closed,
            _ => PrState::Open,
        }
    };

    let review_decision = match response.review_decision.as_deref() {
        Some("APPROVED") => ReviewDecision::Approved,
        Some("CHANGES_REQUESTED") => ReviewDecision::ChangesRequested,
        _ => ReviewDecision::Pending,
    };

    let checks: Vec<(String, CheckStatus)> = response
        .status_check_rollup
        .iter()
        .map(|c| (c.display_name(), c.to_check_status()))
        .collect();

    let checks_status = if checks.is_empty() {
        CheckStatus::Pending
    } else if checks.iter().any(|(_, s)| *s == CheckStatus::Failure) {
        CheckStatus::Failure
    } else if checks.iter().all(|(_, s)| *s == CheckStatus::Success) {
        CheckStatus::Success
    } else {
        CheckStatus::Pending
    };

    PrDetails {
        number: response.number,
        title: response.title,
        url: response.url,
        state,
        additions: response.additions,
        deletions: response.deletions,
        review_decision,
        checks_status,
        checks,
    }
}

/// Fetch rich PR details using `gh pr view`. Returns `None` if `gh` is not
/// installed, the command fails, or no PR exists for the given branch.
pub fn pull_request_details(repo_slug: &str, branch: &str) -> Option<PrDetails> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            "--repo",
            repo_slug,
            "--json",
            "number,title,url,state,isDraft,additions,deletions,reviewDecision,statusCheckRollup",
            branch,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let response: GhPrResponse = serde_json::from_slice(&output.stdout).ok()?;
    Some(parse_pr_details(response))
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

    fn current_user(&self, token: &str) -> Result<GithubUserProfile, String>;
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

    fn current_user(&self, token: &str) -> Result<GithubUserProfile, String> {
        let token = token.to_owned();

        let runtime = tokio::runtime::Runtime::new()
            .map_err(|error| format!("failed to create runtime: {error}"))?;

        runtime.block_on(async move {
            let octocrab = octocrab::Octocrab::builder()
                .personal_token(token)
                .build()
                .map_err(|error| format!("failed to create GitHub client: {error}"))?;

            let user = octocrab
                .current()
                .user()
                .await
                .map_err(|error| format!("failed to fetch GitHub user profile: {error}"))?;

            Ok(GithubUserProfile {
                login: user.login,
                avatar_url: Some(user.avatar_url.to_string()),
            })
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

#[cfg(test)]
mod tests {
    use super::{CheckStatus, GhCheckContext, GhPrResponse, parse_pr_details};

    #[test]
    fn in_progress_checks_remain_pending_without_conclusion() {
        let details = parse_pr_details(GhPrResponse {
            number: 40,
            title: "Fix native toolbar buttons and linked worktree grouping".to_owned(),
            url: "https://github.com/penso/arbor/pull/40".to_owned(),
            state: "OPEN".to_owned(),
            is_draft: false,
            additions: 10,
            deletions: 2,
            review_decision: None,
            status_check_rollup: vec![
                GhCheckContext {
                    name: Some("Clippy".to_owned()),
                    context: None,
                    conclusion: Some(String::new()),
                    status: Some("IN_PROGRESS".to_owned()),
                    state: None,
                },
                GhCheckContext {
                    name: Some("Test".to_owned()),
                    context: None,
                    conclusion: Some(String::new()),
                    status: Some("IN_PROGRESS".to_owned()),
                    state: None,
                },
            ],
        });

        assert_eq!(details.checks_status, CheckStatus::Pending);
        assert_eq!(details.checks, vec![
            ("Clippy".to_owned(), CheckStatus::Pending),
            ("Test".to_owned(), CheckStatus::Pending),
        ]);
    }
}
