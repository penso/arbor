use {
    crate::{domain::Issue, workflow::TrackerConfig},
    async_trait::async_trait,
    serde_json::{Value, json},
    std::time::Duration,
    thiserror::Error,
};

#[derive(Debug, Error, Clone)]
pub enum TrackerError {
    #[error("unsupported_tracker_kind: {0}")]
    UnsupportedTrackerKind(String),
    #[error("missing_tracker_api_key")]
    MissingTrackerApiKey,
    #[error("missing_tracker_project_slug")]
    MissingTrackerProjectSlug,
    #[error("linear_api_request: {0}")]
    LinearApiRequest(String),
    #[error("linear_api_status: {0}")]
    LinearApiStatus(String),
    #[error("linear_graphql_errors: {0}")]
    LinearGraphqlErrors(String),
    #[error("linear_unknown_payload: {0}")]
    LinearUnknownPayload(String),
    #[error("linear_missing_end_cursor")]
    LinearMissingEndCursor,
}

impl From<serde_json::Error> for TrackerError {
    fn from(error: serde_json::Error) -> Self {
        Self::LinearUnknownPayload(error.to_string())
    }
}

impl From<ureq::Error> for TrackerError {
    fn from(error: ureq::Error) -> Self {
        Self::LinearApiRequest(error.to_string())
    }
}

#[async_trait]
pub trait IssueTracker: Send + Sync {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError>;
    async fn fetch_issues_by_states(&self, states: &[String]) -> Result<Vec<Issue>, TrackerError>;
    async fn fetch_issue_states_by_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError>;
}

#[derive(Debug, Clone)]
pub struct LinearTracker {
    config: TrackerConfig,
}

impl LinearTracker {
    pub fn new(config: TrackerConfig) -> Result<Self, TrackerError> {
        if !config.kind.eq_ignore_ascii_case("linear") {
            return Err(TrackerError::UnsupportedTrackerKind(config.kind));
        }
        if config.api_key.trim().is_empty() {
            return Err(TrackerError::MissingTrackerApiKey);
        }
        if config.project_slug.trim().is_empty() {
            return Err(TrackerError::MissingTrackerProjectSlug);
        }
        Ok(Self { config })
    }

    fn request_json(&self, query: &str, variables: Value) -> Result<Value, TrackerError> {
        let body = json!({
            "query": query,
            "variables": variables,
        });

        let payload = serde_json::to_string(&body)
            .map_err(|error| TrackerError::LinearApiRequest(error.to_string()))?;

        let mut response = ureq::post(&self.config.endpoint)
            .header("Authorization", &self.config.api_key)
            .header("User-Agent", "Arbor Symphony")
            .content_type("application/json")
            .config()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .send(&payload)?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.body_mut().read_to_string().unwrap_or_default();
            return Err(TrackerError::LinearApiStatus(format!("{status}: {text}")));
        }

        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|error| TrackerError::LinearApiRequest(error.to_string()))?;
        let value: Value = serde_json::from_str(&text)?;

        if let Some(errors) = value.get("errors") {
            return Err(TrackerError::LinearGraphqlErrors(errors.to_string()));
        }

        Ok(value)
    }

    fn collect_paginated(&self, states: &[String]) -> Result<Vec<Issue>, TrackerError> {
        let mut cursor: Option<String> = None;
        let mut issues = Vec::new();

        loop {
            let response = self.request_json(
                CANDIDATE_ISSUES_QUERY,
                json!({
                    "projectSlug": self.config.project_slug,
                    "states": states,
                    "after": cursor,
                    "first": 50,
                }),
            )?;

            let connection = response.pointer("/data/issues").ok_or_else(|| {
                TrackerError::LinearUnknownPayload("missing data.issues".to_owned())
            })?;
            let nodes = connection
                .get("nodes")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    TrackerError::LinearUnknownPayload("missing issue nodes".to_owned())
                })?;

            for node in nodes {
                issues.push(normalize_issue(node));
            }

            let page_info = connection
                .get("pageInfo")
                .ok_or_else(|| TrackerError::LinearUnknownPayload("missing pageInfo".to_owned()))?;
            let has_next_page = page_info
                .get("hasNextPage")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !has_next_page {
                break;
            }

            let end_cursor = page_info.get("endCursor").and_then(Value::as_str);
            let Some(end_cursor) = end_cursor else {
                return Err(TrackerError::LinearMissingEndCursor);
            };
            cursor = Some(end_cursor.to_owned());
        }

        Ok(issues)
    }
}

#[async_trait]
impl IssueTracker for LinearTracker {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        let states = self.config.active_states.clone();
        let tracker = self.clone();
        tokio::task::spawn_blocking(move || tracker.collect_paginated(&states))
            .await
            .map_err(|error| TrackerError::LinearApiRequest(error.to_string()))?
    }

    async fn fetch_issues_by_states(&self, states: &[String]) -> Result<Vec<Issue>, TrackerError> {
        if states.is_empty() {
            return Ok(Vec::new());
        }

        let states = states.to_vec();
        let tracker = self.clone();
        tokio::task::spawn_blocking(move || tracker.collect_paginated(&states))
            .await
            .map_err(|error| TrackerError::LinearApiRequest(error.to_string()))?
    }

    async fn fetch_issue_states_by_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        if issue_ids.is_empty() {
            return Ok(Vec::new());
        }

        let ids = issue_ids.to_vec();
        let tracker = self.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<Issue>, TrackerError> {
            let response = tracker.request_json(
                ISSUE_STATES_QUERY,
                json!({
                    "ids": ids,
                }),
            )?;

            let nodes = response
                .pointer("/data/issues/nodes")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    TrackerError::LinearUnknownPayload("missing data.issues.nodes".to_owned())
                })?;

            Ok(nodes.iter().map(normalize_issue).collect())
        })
        .await
        .map_err(|error| TrackerError::LinearApiRequest(error.to_string()))?
    }
}

fn normalize_issue(node: &Value) -> Issue {
    let labels = node
        .pointer("/labels/nodes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .map(|value| value.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();

    let blocked_by = node
        .pointer("/inverseRelations/nodes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("blocks"))
                .map(|item| IssueBlocker {
                    id: item
                        .pointer("/issue/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    identifier: item
                        .pointer("/issue/identifier")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    state: item
                        .pointer("/issue/state/name")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                })
                .collect()
        })
        .unwrap_or_default();

    Issue {
        id: string_field(node, "id"),
        identifier: string_field(node, "identifier"),
        title: string_field(node, "title"),
        description: node
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        priority: node.get("priority").and_then(Value::as_i64),
        state: node
            .pointer("/state/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        branch_name: node
            .get("branchName")
            .and_then(Value::as_str)
            .map(str::to_owned),
        url: node.get("url").and_then(Value::as_str).map(str::to_owned),
        labels,
        blocked_by,
        created_at: node
            .get("createdAt")
            .and_then(Value::as_str)
            .map(str::to_owned),
        updated_at: node
            .get("updatedAt")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn string_field(node: &Value, key: &str) -> String {
    node.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

const CANDIDATE_ISSUES_QUERY: &str = r#"
query CandidateIssues($projectSlug: String!, $states: [String!], $after: String, $first: Int!) {
  issues(
    first: $first,
    after: $after,
    filter: {
      project: { slugId: { eq: $projectSlug } }
      state: { name: { in: $states } }
    }
  ) {
    pageInfo { hasNextPage endCursor }
    nodes {
      id
      identifier
      title
      description
      priority
      branchName
      url
      createdAt
      updatedAt
      state { name }
      labels { nodes { name } }
      inverseRelations {
        nodes {
          type
          issue {
            id
            identifier
            state { name }
          }
        }
      }
    }
  }
}
"#;

const ISSUE_STATES_QUERY: &str = r#"
query IssueStatesByIds($ids: [ID!]) {
  issues(filter: { id: { in: $ids } }) {
    nodes {
      id
      identifier
      title
      priority
      branchName
      url
      createdAt
      updatedAt
      state { name }
      labels { nodes { name } }
    }
  }
}
"#;

use crate::domain::IssueBlocker;

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_issue_payload() {
        let issue = normalize_issue(&json!({
            "id": "1",
            "identifier": "ARB-1",
            "title": "Test",
            "description": "Body",
            "priority": 2,
            "branchName": "arb-1",
            "url": "https://example.com",
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-02T00:00:00Z",
            "state": { "name": "Todo" },
            "labels": { "nodes": [{ "name": "Bug" }] },
            "inverseRelations": {
                "nodes": [{
                    "type": "blocks",
                    "issue": {
                        "id": "2",
                        "identifier": "ARB-2",
                        "state": { "name": "In Progress" }
                    }
                }]
            }
        }));

        assert_eq!(issue.labels, vec!["bug"]);
        assert_eq!(issue.blocked_by.len(), 1);
        assert_eq!(issue.state, "Todo");
    }
}
