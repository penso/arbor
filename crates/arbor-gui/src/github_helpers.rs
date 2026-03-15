use super::*;

pub(crate) fn github_repo_slug_for_repo(repo_root: &Path) -> Option<String> {
    let remote_url = git_origin_remote_url(repo_root)?;
    github_repo_slug_from_remote_url(remote_url.trim())
}

pub(crate) fn github_avatar_url_for_repo_slug(repo_slug: &str) -> Option<String> {
    let (owner, _) = repo_slug.split_once('/')?;
    Some(format!(
        "https://avatars.githubusercontent.com/{owner}?size=96"
    ))
}

pub(crate) fn github_repo_url(repo_slug: &str) -> String {
    format!("https://github.com/{repo_slug}")
}

pub(crate) fn github_authenticated_user(
    saved_token: Option<&str>,
) -> Option<(String, Option<String>)> {
    let token = resolve_github_access_token(saved_token)?;
    let response = ureq::get("https://api.github.com/user")
        .header("Authorization", &format!("Bearer {token}"))
        .header("User-Agent", "Arbor")
        .call()
        .ok()?;

    if response.status() != 200 {
        return None;
    }

    let body = response.into_body().read_to_string().ok()?;
    let payload = serde_json::from_str::<serde_json::Value>(&body).ok()?;
    let login = payload
        .get("login")
        .and_then(|value| value.as_str())
        .and_then(non_empty_trimmed_str)
        .map(str::to_owned)?;
    let avatar_url = payload
        .get("avatar_url")
        .and_then(|value| value.as_str())
        .and_then(non_empty_trimmed_str)
        .map(str::to_owned);

    Some((login, avatar_url))
}

pub(crate) fn git_origin_remote_url(repo_root: &Path) -> Option<String> {
    let repo = gix::open(repo_root).ok()?;
    let remote = repo.find_remote("origin").ok()?;
    let url = remote.url(gix::remote::Direction::Fetch)?;
    let url_str = url.to_bstring().to_string();
    if url_str.is_empty() {
        return None;
    }
    Some(url_str)
}

pub(crate) fn github_repo_slug_from_remote_url(remote_url: &str) -> Option<String> {
    if let Some(path) = remote_url.strip_prefix("git@github.com:") {
        return github_repo_slug_from_path(path);
    }

    if let Some(path) = remote_url.strip_prefix("https://github.com/") {
        return github_repo_slug_from_path(path);
    }

    if let Some(path) = remote_url.strip_prefix("http://github.com/") {
        return github_repo_slug_from_path(path);
    }

    if let Some(path) = remote_url.strip_prefix("ssh://git@github.com/") {
        return github_repo_slug_from_path(path);
    }

    None
}

pub(crate) fn github_repo_slug_from_path(path: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let repository_path = normalized.strip_suffix(".git").unwrap_or(normalized);
    let (owner, repository) = repository_path.split_once('/')?;
    if owner.is_empty() || repository.is_empty() {
        return None;
    }

    Some(format!("{owner}/{repository}"))
}

pub(crate) fn github_pr_number_for_worktree(
    github_service: &dyn github_service::GitHubService,
    worktree_path: &Path,
    branch: &str,
    github_token: Option<&str>,
) -> Option<u64> {
    if branch.trim().is_empty() || branch == "-" {
        return None;
    }

    github_pr_number_by_tracking_branch(github_service, worktree_path, github_token).or_else(|| {
        github_pr_number_by_head_branch(github_service, worktree_path, branch, github_token)
    })
}

pub(crate) fn should_lookup_pull_request_for_worktree(worktree: &WorktreeSummary) -> bool {
    if worktree.is_primary_checkout {
        return false;
    }

    let branch = worktree.branch.as_str();
    if branch == "-" || branch.is_empty() {
        return false;
    }

    !(branch.eq_ignore_ascii_case("main")
        || branch.eq_ignore_ascii_case("master")
        || branch.eq_ignore_ascii_case("develop")
        || branch.eq_ignore_ascii_case("dev")
        || branch.eq_ignore_ascii_case("trunk"))
}

pub(crate) fn github_pr_number_by_tracking_branch(
    github_service: &dyn github_service::GitHubService,
    worktree_path: &Path,
    github_token: Option<&str>,
) -> Option<u64> {
    let branch = git_branch_name_for_worktree(worktree_path).ok()?;
    github_pr_number_by_head_branch(github_service, worktree_path, &branch, github_token)
}

pub(crate) fn github_pr_number_by_head_branch(
    github_service: &dyn github_service::GitHubService,
    worktree_path: &Path,
    branch: &str,
    github_token: Option<&str>,
) -> Option<u64> {
    let slug = github_repo_slug_for_repo(worktree_path)?;
    let token = resolve_github_access_token(github_token)?;
    github_service.open_pull_request_number(&slug, branch, &token)
}

pub(crate) fn github_pr_url(repo_slug: &str, pr_number: u64) -> String {
    format!("https://github.com/{repo_slug}/pull/{pr_number}")
}

pub(crate) fn non_empty_trimmed_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub(crate) fn github_access_token_from_env() -> Option<String> {
    env::var("GITHUB_TOKEN")
        .ok()
        .and_then(|value| non_empty_trimmed_str(&value).map(str::to_owned))
}

pub(crate) fn resolve_github_access_token(saved_token: Option<&str>) -> Option<String> {
    let env_token = github_access_token_from_env();
    resolve_github_access_token_from_sources(saved_token, env_token.as_deref())
        .or_else(github_service::github_access_token_from_gh_cli)
}

pub(crate) fn resolve_github_access_token_from_sources(
    saved_token: Option<&str>,
    env_token: Option<&str>,
) -> Option<String> {
    saved_token
        .and_then(non_empty_trimmed_str)
        .map(str::to_owned)
        .or_else(|| env_token.and_then(non_empty_trimmed_str).map(str::to_owned))
}

pub(crate) fn github_oauth_client_id() -> Option<String> {
    env::var("ARBOR_GITHUB_OAUTH_CLIENT_ID")
        .ok()
        .or_else(|| env::var("GITHUB_OAUTH_CLIENT_ID").ok())
        .or_else(|| BUILT_IN_GITHUB_OAUTH_CLIENT_ID.map(str::to_owned))
        .and_then(|value| non_empty_trimmed_str(&value).map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_token_resolution_prefers_saved_token() {
        let token =
            resolve_github_access_token_from_sources(Some(" saved-token "), Some("env-token"));
        assert_eq!(token.as_deref(), Some("saved-token"));
    }

    #[test]
    fn github_token_resolution_falls_back_to_environment_token() {
        let token = resolve_github_access_token_from_sources(Some(""), Some(" env-token "));
        assert_eq!(token.as_deref(), Some("env-token"));
    }
}
