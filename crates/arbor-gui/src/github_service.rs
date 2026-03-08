use std::{
    process::{Command, Stdio},
    sync::Arc,
};

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
