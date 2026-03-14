use async_trait::async_trait;

use super::client::GitHubClient;
use crate::config::schema::TrackerConfig;
use crate::tracker::{Issue, Tracker, TrackerError};

pub struct GitHubAdapter {
    client: GitHubClient,
    config: TrackerConfig,
}

impl GitHubAdapter {
    pub fn new(config: TrackerConfig) -> Self {
        Self {
            client: GitHubClient::new(),
            config,
        }
    }

    /// Check if project-based tracking is enabled and configured.
    fn project_enabled(&self) -> bool {
        self.config.project_number.unwrap_or(0) > 0
    }

    /// Fetch issues from the GitHub Project, using project status as the state.
    /// Falls back to label-based filtering if project is not configured.
    async fn fetch_project_items(
        &self,
        active_states: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        let owner = self
            .config
            .owner
            .as_deref()
            .ok_or(TrackerError::MissingRepo)?;
        let project_number = self.config.project_number.unwrap_or(0);

        let output = tokio::process::Command::new("gh")
            .args([
                "project",
                "item-list",
                &project_number.to_string(),
                "--owner",
                owner,
                "--format",
                "json",
                "--limit",
                "100",
            ])
            .output()
            .await
            .map_err(|e| TrackerError::ApiRequest(format!("gh project item-list failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TrackerError::ApiRequest(format!(
                "gh project item-list returned {}: {stderr}",
                output.status
            )));
        }

        let json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| TrackerError::UnknownPayload(format!("project JSON parse error: {e}")))?;

        let items = json
            .get("items")
            .and_then(|i| i.as_array())
            .ok_or_else(|| {
                TrackerError::UnknownPayload("no items array in project response".into())
            })?;

        let active_lower: Vec<String> = active_states.iter().map(|s| s.to_lowercase()).collect();
        let repo_name = self.config.repo.as_deref().unwrap_or("repo");

        let mut issues = Vec::new();
        for item in items {
            let status = item.get("status").and_then(|s| s.as_str()).unwrap_or("");
            let content = item.get("content");
            let item_type = content
                .and_then(|c| c.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            // Only process Issues (not PRs or drafts)
            if item_type != "Issue" {
                continue;
            }

            // Filter by active project states
            if !active_lower.contains(&status.to_lowercase()) {
                continue;
            }

            let number = content
                .and_then(|c| c.get("number"))
                .and_then(|n| n.as_u64())
                .unwrap_or(0);
            let title = content
                .and_then(|c| c.get("title"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            let body = content
                .and_then(|c| c.get("body"))
                .and_then(|b| b.as_str())
                .map(|s| s.to_string());
            let url = content
                .and_then(|c| c.get("url"))
                .and_then(|u| u.as_str())
                .map(|s| s.to_string());

            let labels: Vec<String> = item
                .get("labels")
                .and_then(|l| l.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_lowercase())
                        .collect()
                })
                .unwrap_or_default();

            if number == 0 {
                continue;
            }

            issues.push(Issue {
                id: number.to_string(),
                identifier: format!("{repo_name}-{number}"),
                title,
                description: body,
                priority: labels.iter().find_map(|l| {
                    l.strip_prefix("priority-")
                        .and_then(|n| n.parse::<i32>().ok())
                }),
                state: status.to_string(), // Use project status as state
                branch_name: None,
                url,
                labels,
                blocked_by: vec![],
                created_at: None,
                updated_at: None,
            });
        }

        Ok(issues)
    }
}

#[async_trait]
impl Tracker for GitHubAdapter {
    async fn fetch_candidate_issues(
        &self,
        config: &TrackerConfig,
    ) -> Result<Vec<Issue>, TrackerError> {
        // If project is enabled, use project status for state tracking
        if self.project_enabled() {
            let active_states = config.effective_active_states();
            return self.fetch_project_items(&active_states).await;
        }

        // Fallback: label-based filtering
        if !config.active_issue_labels.is_empty() {
            let all = self.client.fetch_issues(config, "open", None).await?;
            let required: Vec<String> = config
                .active_issue_labels
                .iter()
                .map(|l| l.to_lowercase())
                .collect();
            Ok(all
                .into_iter()
                .filter(|issue| {
                    issue
                        .labels
                        .iter()
                        .any(|l| required.contains(&l.to_lowercase()))
                })
                .collect())
        } else {
            let labels = if config.labels.is_empty() {
                None
            } else {
                Some(config.labels.as_slice())
            };
            self.client.fetch_issues(config, "open", labels).await
        }
    }

    async fn fetch_issue_states_by_ids(&self, ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        // Always use REST API for reconciliation — cheaper and supports ETags.
        // Project API is too expensive for per-tick state checks.
        let numbers: Vec<u64> = ids.iter().filter_map(|id| id.parse::<u64>().ok()).collect();
        self.client
            .fetch_issues_by_numbers(&self.config, &numbers)
            .await
    }

    async fn fetch_issues_by_states(
        &self,
        states: &[String],
        config: &TrackerConfig,
    ) -> Result<Vec<Issue>, TrackerError> {
        if self.project_enabled() {
            return self.fetch_project_items(states).await;
        }

        // Fallback: REST API
        let github_state = if states
            .iter()
            .any(|state| state.eq_ignore_ascii_case("closed"))
        {
            "closed"
        } else {
            "open"
        };

        let all = self.client.fetch_issues(config, github_state, None).await?;
        let requested: Vec<String> = states.iter().map(|s| s.to_lowercase()).collect();
        Ok(all
            .into_iter()
            .filter(|issue| requested.contains(&issue.state.to_lowercase()))
            .collect())
    }
}
