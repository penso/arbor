use super::*;

/// How often to check for a new version (4 hours).
const VERSION_CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

/// GitHub API endpoint for the latest release.
const GITHUB_LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/penso/arbor/releases/latest";

/// Fetch the latest release tag from GitHub.
///
/// Returns the tag name (e.g. "v0.2.0") or an error.
fn fetch_latest_release_tag() -> anyhow::Result<String> {
    let config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let response = agent
        .get(GITHUB_LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", &format!("Arbor/{APP_VERSION}"))
        .call()?;

    let status: u16 = response.status().into();
    if status != 200 {
        anyhow::bail!("GitHub API returned status {status}");
    }

    let body_bytes = response.into_body().read_to_vec()?;
    let body: serde_json::Value = serde_json::from_slice(&body_bytes)?;
    let tag = body["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing tag_name in GitHub release response"))?;

    Ok(tag.to_owned())
}

/// Strip a leading 'v' from a version tag (e.g. "v0.2.0" → "0.2.0").
fn strip_version_prefix(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// Compare two semver-like version strings. Returns true if `latest` is newer
/// than `current`. Falls back to lexicographic comparison if parsing fails.
fn is_newer_version(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> Option<(u64, u64, u64)> {
        let mut parts = s.splitn(3, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        // Pre-release suffix (e.g. "0-rc1") is stripped by taking only digits.
        let patch_str = parts.next().unwrap_or("0");
        let patch: u64 = patch_str
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        Some((major, minor, patch))
    };

    match (parse(current), parse(latest)) {
        (Some(cur), Some(lat)) => lat > cur,
        _ => latest > current,
    }
}

impl ArborWindow {
    /// Start a background poller that checks GitHub for new releases.
    pub(crate) fn start_version_check_poller(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // Small initial delay so startup isn't slowed down.
            cx.background_spawn(async move {
                std::thread::sleep(Duration::from_secs(10));
            })
            .await;

            loop {
                let result = cx
                    .background_spawn(async move { fetch_latest_release_tag() })
                    .await;

                let updated = this.update(cx, |this, cx| {
                    match result {
                        Ok(tag) => {
                            let latest = strip_version_prefix(&tag);
                            if is_newer_version(APP_VERSION, latest) {
                                tracing::info!(
                                    current = APP_VERSION,
                                    latest,
                                    "new Arbor version available"
                                );
                                this.update_available = Some(latest.to_owned());
                                cx.notify();
                            } else {
                                tracing::debug!(
                                    current = APP_VERSION,
                                    latest,
                                    "Arbor is up to date"
                                );
                                // Clear any stale banner if versions match now.
                                if this.update_available.is_some() {
                                    this.update_available = None;
                                    cx.notify();
                                }
                            }
                        },
                        Err(e) => {
                            tracing::debug!("version check failed: {e:#}");
                        },
                    }
                });

                if updated.is_err() {
                    break;
                }

                cx.background_spawn(async move {
                    std::thread::sleep(VERSION_CHECK_INTERVAL);
                })
                .await;
            }
        })
        .detach();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn strip_prefix_handles_v_and_bare() {
        assert_eq!(strip_version_prefix("v1.2.3"), "1.2.3");
        assert_eq!(strip_version_prefix("1.2.3"), "1.2.3");
    }

    #[test]
    fn newer_version_detected() {
        assert!(is_newer_version("0.1.0", "0.2.0"));
        assert!(is_newer_version("0.1.0", "1.0.0"));
        assert!(is_newer_version("0.1.0", "0.1.1"));
    }

    #[test]
    fn same_version_is_not_newer() {
        assert!(!is_newer_version("0.1.0", "0.1.0"));
    }

    #[test]
    fn older_version_is_not_newer() {
        assert!(!is_newer_version("0.2.0", "0.1.0"));
        assert!(!is_newer_version("1.0.0", "0.9.9"));
    }

    #[test]
    fn pre_release_suffix_handled() {
        assert!(is_newer_version("0.1.0", "0.2.0-rc1"));
    }
}
