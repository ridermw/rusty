use std::sync::RwLock;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use tracing::{info, warn};

use super::client::GitHubClient;
use crate::config::{resolve_github_token, schema::TrackerConfig};
use crate::tracker::{Issue, Tracker, TrackerError};

const INITIAL_GRAPHQL_BACKOFF_SECS: u64 = 60;
const MAX_GRAPHQL_BACKOFF_SECS: u64 = 15 * 60;

pub struct GitHubAdapter {
    client: GitHubClient,
    config: TrackerConfig,
    /// Cached project items from the last successful GraphQL fetch.
    project_cache: RwLock<Option<Vec<Issue>>>,
    /// ETag for the tier-1 REST change detection request.
    change_etag: RwLock<Option<String>>,
    /// Exponential backoff state for GraphQL rate limits.
    graphql_backoff: RwLock<BackoffState>,
}

#[derive(Debug, Clone)]
struct BackoffState {
    /// Next allowed GraphQL call time.
    next_allowed: Option<DateTime<Utc>>,
    /// Current backoff duration in seconds.
    backoff_secs: u64,
    /// Number of consecutive rate limit errors.
    consecutive_errors: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeCheck {
    Changed,
    Unchanged,
}

impl BackoffState {
    fn new() -> Self {
        Self {
            next_allowed: None,
            backoff_secs: 0,
            consecutive_errors: 0,
        }
    }

    fn is_backing_off(&self) -> bool {
        self.next_allowed
            .is_some_and(|next_allowed| Utc::now() < next_allowed)
    }

    fn record_success(&mut self) {
        self.next_allowed = None;
        self.backoff_secs = 0;
        self.consecutive_errors = 0;
    }

    fn record_rate_limit(&mut self, reset_at: Option<DateTime<Utc>>) {
        self.consecutive_errors = self.consecutive_errors.saturating_add(1);
        self.backoff_secs = if self.backoff_secs == 0 {
            INITIAL_GRAPHQL_BACKOFF_SECS
        } else {
            (self.backoff_secs.saturating_mul(2)).min(MAX_GRAPHQL_BACKOFF_SECS)
        };

        let now = Utc::now();
        let exponential_next = now + Duration::seconds(self.backoff_secs as i64);
        self.next_allowed = Some(match reset_at {
            Some(reset_at) if reset_at > exponential_next => reset_at,
            _ => exponential_next,
        });
    }

    fn next_allowed(&self) -> Option<DateTime<Utc>> {
        self.next_allowed
            .filter(|next_allowed| Utc::now() < *next_allowed)
    }

    fn remaining_backoff_secs(&self) -> Option<u64> {
        self.next_allowed().map(|next_allowed| {
            (next_allowed - Utc::now())
                .num_seconds()
                .max(0)
                .try_into()
                .unwrap_or(0)
        })
    }
}

impl GitHubAdapter {
    pub fn new(config: TrackerConfig) -> Self {
        Self {
            client: GitHubClient::new(),
            config,
            project_cache: RwLock::new(None),
            change_etag: RwLock::new(None),
            graphql_backoff: RwLock::new(BackoffState::new()),
        }
    }

    /// Check if project-based tracking is enabled and configured.
    fn project_enabled(&self) -> bool {
        self.config.project_number.unwrap_or(0) > 0
    }

    fn repo_name(config: &TrackerConfig) -> String {
        config
            .full_repo()
            .as_deref()
            .and_then(|repo| repo.split('/').next_back())
            .or(config.repo.as_deref())
            .unwrap_or("repo")
            .to_string()
    }

    fn change_detection_url(config: &TrackerConfig) -> Result<String, TrackerError> {
        let repo = config.full_repo().ok_or(TrackerError::MissingRepo)?;
        let endpoint = config
            .endpoint
            .as_deref()
            .unwrap_or("https://api.github.com")
            .trim_end_matches('/');
        Ok(format!(
            "{endpoint}/repos/{repo}/issues?state=open&per_page=1&sort=updated&direction=desc"
        ))
    }

    fn cached_project_items(&self, requested_states: &[String]) -> Option<Vec<Issue>> {
        self.project_cache
            .read()
            .unwrap()
            .as_ref()
            .map(|items| Self::filter_project_items(items, requested_states))
    }

    fn update_change_etag(&self, response: &reqwest::Response) {
        if let Some(etag) = response.headers().get("etag") {
            if let Ok(etag) = etag.to_str() {
                *self.change_etag.write().unwrap() = Some(etag.to_string());
            }
        }
    }

    fn filter_project_items(items: &[Issue], requested_states: &[String]) -> Vec<Issue> {
        if requested_states.is_empty() {
            return items.to_vec();
        }

        let requested: Vec<String> = requested_states
            .iter()
            .map(|state| state.to_lowercase())
            .collect();
        items
            .iter()
            .filter(|issue| requested.contains(&issue.state.to_lowercase()))
            .cloned()
            .collect()
    }

    async fn check_for_changes(&self) -> Result<ChangeCheck, TrackerError> {
        let token = resolve_github_token(self.config.api_key.as_deref())
            .await
            .map_err(|_| TrackerError::MissingApiKey)?;
        let url = Self::change_detection_url(&self.config)?;

        let mut request = reqwest::Client::new()
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "rusty/0.1");

        if let Some(etag) = self.change_etag.read().unwrap().clone() {
            request = request.header("If-None-Match", etag);
        }

        let response = request
            .send()
            .await
            .map_err(|err| TrackerError::ApiRequest(err.to_string()))?;

        match response.status() {
            StatusCode::NOT_MODIFIED => {
                self.update_change_etag(&response);
                Ok(ChangeCheck::Unchanged)
            }
            StatusCode::OK => {
                self.update_change_etag(&response);
                Ok(ChangeCheck::Changed)
            }
            StatusCode::TOO_MANY_REQUESTS => {
                let reset_at = response
                    .headers()
                    .get("x-ratelimit-reset")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<i64>().ok())
                    .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
                    .unwrap_or_else(Utc::now);
                Err(TrackerError::RateLimited { reset_at })
            }
            status => {
                let body = response.text().await.unwrap_or_default();
                Err(TrackerError::ApiStatus(status.as_u16(), body))
            }
        }
    }

    async fn fetch_project_items(
        &self,
        requested_states: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        let active_backoff = {
            let backoff = self.graphql_backoff.read().unwrap();
            if backoff.is_backing_off() {
                backoff.next_allowed()
            } else {
                None
            }
        };
        if let Some(reset_at) = active_backoff {
            if let Some(cached) = self.cached_project_items(requested_states) {
                info!(
                    item_count = cached.len(),
                    ?reset_at,
                    "graphql backoff active, using cached project items"
                );
                return Ok(cached);
            }
            return Err(TrackerError::RateLimited { reset_at });
        }

        match self.check_for_changes().await {
            Ok(ChangeCheck::Unchanged) => {
                if let Some(cached) = self.cached_project_items(requested_states) {
                    info!(
                        item_count = cached.len(),
                        "tier1 change check: 304 no change, using cached project items"
                    );
                    return Ok(cached);
                }
                info!("tier1 change check: 304 no change but cache empty, fetching project items");
            }
            Ok(ChangeCheck::Changed) => {
                info!("tier1 change check: 200 changes detected, fetching project items");
            }
            Err(TrackerError::RateLimited { reset_at }) => {
                if let Some(cached) = self.cached_project_items(requested_states) {
                    warn!(
                        item_count = cached.len(),
                        ?reset_at,
                        "tier1 change check rate limited, using cached project items"
                    );
                    return Ok(cached);
                }
                return Err(TrackerError::RateLimited { reset_at });
            }
            Err(err) => return Err(err),
        }

        let all_items = match self.fetch_project_items_graphql().await {
            Ok(items) => items,
            Err(TrackerError::RateLimited { reset_at }) => {
                let (next_allowed, backoff_secs, attempt) = {
                    let mut backoff = self.graphql_backoff.write().unwrap();
                    backoff.record_rate_limit(Some(reset_at));
                    (
                        backoff.next_allowed.unwrap_or(reset_at),
                        backoff
                            .remaining_backoff_secs()
                            .unwrap_or(backoff.backoff_secs),
                        backoff.consecutive_errors,
                    )
                };
                warn!(
                    backoff_secs,
                    attempt,
                    ?next_allowed,
                    "graphql rate limited, backing off"
                );

                if let Some(cached) = self.cached_project_items(requested_states) {
                    return Ok(cached);
                }

                return Err(TrackerError::RateLimited {
                    reset_at: next_allowed,
                });
            }
            Err(err) => return Err(err),
        };

        let cleared_backoff = {
            let backoff = self.graphql_backoff.read().unwrap();
            backoff.consecutive_errors > 0 || backoff.next_allowed.is_some()
        };
        if cleared_backoff {
            info!("graphql backoff cleared after successful fetch");
        }
        self.graphql_backoff.write().unwrap().record_success();
        *self.project_cache.write().unwrap() = Some(all_items.clone());

        Ok(Self::filter_project_items(&all_items, requested_states))
    }

    async fn fetch_project_items_graphql(&self) -> Result<Vec<Issue>, TrackerError> {
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
            if Self::is_graphql_rate_limited(&stderr) {
                return Err(TrackerError::RateLimited {
                    reset_at: Self::extract_rate_limit_reset(&stderr).unwrap_or_else(Utc::now),
                });
            }

            return Err(TrackerError::ApiRequest(format!(
                "gh project item-list returned {}: {stderr}",
                output.status
            )));
        }

        let json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| TrackerError::UnknownPayload(format!("project JSON parse error: {e}")))?;

        Self::parse_project_items(&json, &self.config)
    }

    fn parse_project_items(
        json: &serde_json::Value,
        config: &TrackerConfig,
    ) -> Result<Vec<Issue>, TrackerError> {
        let items = json
            .get("items")
            .and_then(|items| items.as_array())
            .ok_or_else(|| {
                TrackerError::UnknownPayload("no items array in project response".into())
            })?;

        let repo_name = Self::repo_name(config);
        let mut issues = Vec::new();

        for item in items {
            let status = item
                .get("status")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let content = item.get("content");
            let item_type = content
                .and_then(|content| content.get("type"))
                .and_then(|value| value.as_str())
                .unwrap_or("");

            if item_type != "Issue" {
                continue;
            }

            let number = content
                .and_then(|content| content.get("number"))
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            if number == 0 {
                continue;
            }

            let title = content
                .and_then(|content| content.get("title"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let body = content
                .and_then(|content| content.get("body"))
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let url = content
                .and_then(|content| content.get("url"))
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let labels: Vec<String> = item
                .get("labels")
                .and_then(|value| value.as_array())
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(|value| value.as_str())
                        .map(|value| value.to_lowercase())
                        .collect()
                })
                .unwrap_or_default();

            issues.push(Issue {
                id: number.to_string(),
                identifier: format!("{repo_name}-{number}"),
                title,
                description: body,
                priority: labels.iter().find_map(|label| {
                    label
                        .strip_prefix("priority-")
                        .and_then(|value| value.parse::<i32>().ok())
                }),
                state: status.to_string(),
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

    fn is_graphql_rate_limited(stderr: &str) -> bool {
        let stderr = stderr.to_lowercase();
        stderr.contains("rate limit") || stderr.contains("secondary rate limit")
    }

    fn extract_rate_limit_reset(stderr: &str) -> Option<DateTime<Utc>> {
        for line in stderr.lines() {
            let lower = line.to_lowercase();
            if !lower.contains("x-ratelimit-reset") {
                continue;
            }

            for token in line
                .split(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '=' | ',' | ';' | '"'))
            {
                let token = token.trim();
                if token.is_empty() || token.eq_ignore_ascii_case("x-ratelimit-reset") {
                    continue;
                }

                if let Ok(timestamp) = token.parse::<i64>() {
                    if let Some(reset_at) = DateTime::<Utc>::from_timestamp(timestamp, 0) {
                        return Some(reset_at);
                    }
                }

                if let Ok(reset_at) = token.parse::<DateTime<Utc>>() {
                    return Some(reset_at);
                }
            }
        }

        None
    }
}

#[async_trait]
impl Tracker for GitHubAdapter {
    async fn fetch_candidate_issues(
        &self,
        config: &TrackerConfig,
    ) -> Result<Vec<Issue>, TrackerError> {
        if self.project_enabled() {
            let active_states = config.effective_active_states();
            return self.fetch_project_items(&active_states).await;
        }

        if !config.active_issue_labels.is_empty() {
            let all = self.client.fetch_issues(config, "open", None).await?;
            let required: Vec<String> = config
                .active_issue_labels
                .iter()
                .map(|label| label.to_lowercase())
                .collect();
            Ok(all
                .into_iter()
                .filter(|issue| {
                    issue
                        .labels
                        .iter()
                        .any(|label| required.contains(&label.to_lowercase()))
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

        let github_state = if states
            .iter()
            .any(|state| state.eq_ignore_ascii_case("closed"))
        {
            "closed"
        } else {
            "open"
        };

        let all = self.client.fetch_issues(config, github_state, None).await?;
        let requested: Vec<String> = states.iter().map(|state| state.to_lowercase()).collect();
        Ok(all
            .into_iter()
            .filter(|issue| requested.contains(&issue.state.to_lowercase()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker_config() -> TrackerConfig {
        TrackerConfig {
            owner: Some("octo-org".to_string()),
            repo: Some("rusty".to_string()),
            ..TrackerConfig::default()
        }
    }

    #[test]
    fn backoff_state_starts_clear() {
        let backoff = BackoffState::new();
        assert_eq!(backoff.backoff_secs, 0);
        assert_eq!(backoff.consecutive_errors, 0);
        assert!(backoff.next_allowed.is_none());
        assert!(!backoff.is_backing_off());
    }

    #[test]
    fn backoff_state_record_success_resets_state() {
        let mut backoff = BackoffState {
            next_allowed: Some(Utc::now() + Duration::seconds(90)),
            backoff_secs: 120,
            consecutive_errors: 3,
        };

        backoff.record_success();

        assert_eq!(backoff.backoff_secs, 0);
        assert_eq!(backoff.consecutive_errors, 0);
        assert!(backoff.next_allowed.is_none());
        assert!(!backoff.is_backing_off());
    }

    #[test]
    fn backoff_state_record_rate_limit_increases_exponentially() {
        let mut backoff = BackoffState::new();

        backoff.record_rate_limit(None);
        assert_eq!(backoff.backoff_secs, 60);
        assert_eq!(backoff.consecutive_errors, 1);
        assert!(backoff.is_backing_off());

        backoff.record_rate_limit(None);
        assert_eq!(backoff.backoff_secs, 120);
        assert_eq!(backoff.consecutive_errors, 2);

        backoff.record_rate_limit(None);
        assert_eq!(backoff.backoff_secs, 240);
        assert_eq!(backoff.consecutive_errors, 3);
    }

    #[test]
    fn backoff_state_caps_maximum_duration() {
        let mut backoff = BackoffState::new();

        for _ in 0..8 {
            backoff.record_rate_limit(None);
        }

        assert_eq!(backoff.backoff_secs, MAX_GRAPHQL_BACKOFF_SECS);
        assert_eq!(backoff.consecutive_errors, 8);
        assert!(backoff.is_backing_off());
    }

    #[test]
    fn backoff_state_reports_when_backoff_has_expired() {
        let backoff = BackoffState {
            next_allowed: Some(Utc::now() - Duration::seconds(1)),
            backoff_secs: 60,
            consecutive_errors: 1,
        };
        assert!(!backoff.is_backing_off());

        let backing_off = BackoffState {
            next_allowed: Some(Utc::now() + Duration::seconds(1)),
            backoff_secs: 60,
            consecutive_errors: 1,
        };
        assert!(backing_off.is_backing_off());
    }

    #[test]
    fn change_detection_url_matches_expected_endpoint() {
        let config = TrackerConfig {
            endpoint: Some("https://api.github.com/".to_string()),
            ..tracker_config()
        };

        let url = GitHubAdapter::change_detection_url(&config).unwrap();

        assert_eq!(
            url,
            "https://api.github.com/repos/octo-org/rusty/issues?state=open&per_page=1&sort=updated&direction=desc"
        );
    }
}
