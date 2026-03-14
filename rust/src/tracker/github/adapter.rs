use std::sync::RwLock;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use tracing::{info, warn};

use super::client::GitHubClient;
use crate::config::{resolve_github_token, schema::TrackerConfig};
use crate::ports::{
    HttpClient, ProcessRunner, ReqwestHttpClient, TokioProcessRunner,
};
use crate::tracker::{Issue, Tracker, TrackerError};

const INITIAL_GRAPHQL_BACKOFF_SECS: u64 = 60;
const MAX_GRAPHQL_BACKOFF_SECS: u64 = 15 * 60;
/// Project status changes (e.g. dragging to "Todo" on the board) don't touch
/// the issue itself, so the REST ETag never changes.  Force a full GraphQL
/// refresh at least this often regardless of the tier-1 result.
const CACHE_TTL_SECS: i64 = 120;

pub struct GitHubAdapter<H: HttpClient = ReqwestHttpClient, P: ProcessRunner = TokioProcessRunner> {
    client: GitHubClient<H>,
    http: H,
    process: P,
    config: TrackerConfig,
    /// Cached project items from the last successful GraphQL fetch.
    project_cache: RwLock<Option<Vec<Issue>>>,
    /// ETag for the tier-1 REST change detection request.
    change_etag: RwLock<Option<String>>,
    /// Exponential backoff state for GraphQL rate limits.
    graphql_backoff: RwLock<BackoffState>,
    /// Timestamp of last successful GraphQL fetch (for cache TTL).
    last_graphql_fetch: RwLock<Option<DateTime<Utc>>>,
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
        let http = ReqwestHttpClient::new();
        Self {
            client: GitHubClient::with_http(http.clone()),
            http,
            process: TokioProcessRunner,
            config,
            project_cache: RwLock::new(None),
            change_etag: RwLock::new(None),
            graphql_backoff: RwLock::new(BackoffState::new()),
            last_graphql_fetch: RwLock::new(None),
        }
    }
}

impl<H: HttpClient + Clone, P: ProcessRunner> GitHubAdapter<H, P> {
    pub fn with_deps(config: TrackerConfig, http: H, process: P) -> Self {
        Self {
            client: GitHubClient::with_http(http.clone()),
            http,
            process,
            config,
            project_cache: RwLock::new(None),
            change_etag: RwLock::new(None),
            graphql_backoff: RwLock::new(BackoffState::new()),
            last_graphql_fetch: RwLock::new(None),
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

    fn update_change_etag_from_response(&self, response: &crate::ports::HttpResponse) {
        if let Some(etag) = response.header("etag") {
            *self.change_etag.write().unwrap() = Some(etag.to_string());
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

    /// Returns true if the project cache is older than CACHE_TTL_SECS.
    fn cache_is_stale(&self) -> bool {
        self.last_graphql_fetch
            .read()
            .unwrap()
            .map(|t| (Utc::now() - t).num_seconds() >= CACHE_TTL_SECS)
            .unwrap_or(true) // no fetch yet = stale
    }

    async fn check_for_changes(&self) -> Result<ChangeCheck, TrackerError> {
        let token = resolve_github_token(self.config.api_key.as_deref())
            .await
            .map_err(|_| TrackerError::MissingApiKey)?;
        let url = Self::change_detection_url(&self.config)?;
        let bearer = format!("Bearer {token}");

        let mut headers = vec![
            ("Authorization", bearer.as_str()),
            ("Accept", "application/vnd.github+json"),
            ("User-Agent", "rusty/0.1"),
        ];

        let etag_value = self.change_etag.read().unwrap().clone();
        if let Some(ref etag) = etag_value {
            headers.push(("If-None-Match", etag.as_str()));
        }

        let response = self
            .http
            .get(&url, &headers)
            .await
            .map_err(|err| TrackerError::ApiRequest(err.to_string()))?;

        match response.status {
            304 => {
                self.update_change_etag_from_response(&response);
                Ok(ChangeCheck::Unchanged)
            }
            200 => {
                self.update_change_etag_from_response(&response);
                Ok(ChangeCheck::Changed)
            }
            429 => {
                let reset_at = response
                    .header("x-ratelimit-reset")
                    .and_then(|value| value.parse::<i64>().ok())
                    .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
                    .unwrap_or_else(Utc::now);
                Err(TrackerError::RateLimited { reset_at })
            }
            status => {
                let body = response.text();
                Err(TrackerError::ApiStatus(status, body))
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
            Ok(ChangeCheck::Unchanged) if !self.cache_is_stale() => {
                if let Some(cached) = self.cached_project_items(requested_states) {
                    info!(
                        item_count = cached.len(),
                        "tier1 change check: 304 no change, using cached project items"
                    );
                    return Ok(cached);
                }
                info!("tier1 change check: 304 no change but cache empty, fetching project items");
            }
            Ok(ChangeCheck::Unchanged) => {
                info!(
                    ttl_secs = CACHE_TTL_SECS,
                    "tier1 change check: 304 but cache stale, forcing GraphQL refresh"
                );
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
        *self.last_graphql_fetch.write().unwrap() = Some(Utc::now());

        Ok(Self::filter_project_items(&all_items, requested_states))
    }

    async fn fetch_project_items_graphql(&self) -> Result<Vec<Issue>, TrackerError> {
        let owner = self
            .config
            .owner
            .as_deref()
            .ok_or(TrackerError::MissingRepo)?;
        let project_number = self.config.project_number.unwrap_or(0);
        let number_str = project_number.to_string();

        let output = self
            .process
            .run(
                "gh",
                &[
                    "project",
                    "item-list",
                    &number_str,
                    "--owner",
                    owner,
                    "--format",
                    "json",
                    "--limit",
                    "100",
                ],
            )
            .await
            .map_err(|e| TrackerError::ApiRequest(format!("gh project item-list failed: {e}")))?;

        if !output.status_success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if Self::is_graphql_rate_limited(&stderr) {
                return Err(TrackerError::RateLimited {
                    reset_at: Self::extract_rate_limit_reset(&stderr).unwrap_or_else(Utc::now),
                });
            }

            let status_display = output
                .status_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            return Err(TrackerError::ApiRequest(format!(
                "gh project item-list returned {status_display}: {stderr}",
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
impl<H: HttpClient + Clone, P: ProcessRunner> Tracker for GitHubAdapter<H, P> {
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

    async fn save_session_id(
        &self,
        issue_id: &str,
        session_id: &str,
    ) -> Result<(), TrackerError> {
        let token = crate::config::resolve_github_token(self.config.api_key.as_deref())
            .await
            .map_err(|_| TrackerError::MissingApiKey)?;
        let repo = self.config.full_repo().ok_or(TrackerError::MissingRepo)?;
        let endpoint = self
            .config
            .endpoint
            .as_deref()
            .unwrap_or("https://api.github.com")
            .trim_end_matches('/');

        // Delete any existing session comment first to avoid duplicates
        let _ = self.delete_session_id(issue_id).await;

        let body = serde_json::json!({
            "body": format!("<!-- rusty:session:{session_id} -->"),
        });
        let url = format!("{endpoint}/repos/{repo}/issues/{issue_id}/comments");
        let bearer = format!("Bearer {token}");
        let response = self
            .http
            .post(
                &url,
                &[
                    ("Authorization", bearer.as_str()),
                    ("Accept", "application/vnd.github+json"),
                    ("User-Agent", "rusty/0.1"),
                ],
                Some(body.to_string().as_bytes()),
            )
            .await
            .map_err(|err| TrackerError::ApiRequest(err.to_string()))?;

        if response.status != 201 {
            return Err(TrackerError::ApiStatus(
                response.status,
                response.text(),
            ));
        }

        info!(
            %issue_id,
            %session_id,
            "saved session ID as issue comment"
        );
        Ok(())
    }

    async fn load_session_id(&self, issue_id: &str) -> Result<Option<String>, TrackerError> {
        let token = crate::config::resolve_github_token(self.config.api_key.as_deref())
            .await
            .map_err(|_| TrackerError::MissingApiKey)?;
        let repo = self.config.full_repo().ok_or(TrackerError::MissingRepo)?;
        let endpoint = self
            .config
            .endpoint
            .as_deref()
            .unwrap_or("https://api.github.com")
            .trim_end_matches('/');

        let url = format!("{endpoint}/repos/{repo}/issues/{issue_id}/comments?per_page=100");
        let bearer = format!("Bearer {token}");
        let response = self
            .http
            .get(
                &url,
                &[
                    ("Authorization", bearer.as_str()),
                    ("Accept", "application/vnd.github+json"),
                    ("User-Agent", "rusty/0.1"),
                ],
            )
            .await
            .map_err(|err| TrackerError::ApiRequest(err.to_string()))?;

        if response.status != 200 {
            return Err(TrackerError::ApiStatus(
                response.status,
                response.text(),
            ));
        }

        let comments: Vec<serde_json::Value> = response
            .json()
            .map_err(|err| TrackerError::UnknownPayload(err.to_string()))?;

        for comment in &comments {
            if let Some(body) = comment.get("body").and_then(|b| b.as_str()) {
                if let Some(session_id) = extract_session_marker(body) {
                    return Ok(Some(session_id.to_string()));
                }
            }
        }

        Ok(None)
    }

    async fn delete_session_id(&self, issue_id: &str) -> Result<(), TrackerError> {
        let token = crate::config::resolve_github_token(self.config.api_key.as_deref())
            .await
            .map_err(|_| TrackerError::MissingApiKey)?;
        let repo = self.config.full_repo().ok_or(TrackerError::MissingRepo)?;
        let endpoint = self
            .config
            .endpoint
            .as_deref()
            .unwrap_or("https://api.github.com")
            .trim_end_matches('/');

        let url = format!("{endpoint}/repos/{repo}/issues/{issue_id}/comments?per_page=100");
        let bearer = format!("Bearer {token}");
        let response = self
            .http
            .get(
                &url,
                &[
                    ("Authorization", bearer.as_str()),
                    ("Accept", "application/vnd.github+json"),
                    ("User-Agent", "rusty/0.1"),
                ],
            )
            .await
            .map_err(|err| TrackerError::ApiRequest(err.to_string()))?;

        if response.status != 200 {
            return Ok(()); // best-effort
        }

        let comments: Vec<serde_json::Value> = response
            .json()
            .map_err(|err| TrackerError::UnknownPayload(err.to_string()))?;

        for comment in &comments {
            let body = comment.get("body").and_then(|b| b.as_str()).unwrap_or("");
            if extract_session_marker(body).is_some() {
                if let Some(comment_id) = comment.get("id").and_then(|id| id.as_u64()) {
                    let delete_url =
                        format!("{endpoint}/repos/{repo}/issues/comments/{comment_id}");
                    let _ = self
                        .http
                        .post(
                            &delete_url,
                            &[
                                ("Authorization", bearer.as_str()),
                                ("Accept", "application/vnd.github+json"),
                                ("User-Agent", "rusty/0.1"),
                                ("X-HTTP-Method-Override", "DELETE"),
                            ],
                            None,
                        )
                        .await;
                    info!(%issue_id, %comment_id, "deleted session comment");
                }
            }
        }

        Ok(())
    }
}

/// Session marker prefix used in GitHub issue comments.
const SESSION_MARKER_PREFIX: &str = "<!-- rusty:session:";
const SESSION_MARKER_SUFFIX: &str = " -->";

/// Extract session ID from a comment body containing the marker.
pub fn extract_session_marker(body: &str) -> Option<&str> {
    let trimmed = body.trim();
    let rest = trimmed.strip_prefix(SESSION_MARKER_PREFIX)?;
    rest.strip_suffix(SESSION_MARKER_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::memory::test_issue;
    use serde_json::json;

    // Concrete type alias avoids inference issues with async_trait impls.
    type Adapter = GitHubAdapter<ReqwestHttpClient, TokioProcessRunner>;

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

        let url = <GitHubAdapter>::change_detection_url(&config).unwrap();

        assert_eq!(
            url,
            "https://api.github.com/repos/octo-org/rusty/issues?state=open&per_page=1&sort=updated&direction=desc"
        );
    }

    // ── parse_project_items tests ──────────────────────────────────────

    #[test]
    fn parse_project_items_extracts_valid_issues() {
        let json = json!({
            "items": [
                {
                    "status": "Todo",
                    "labels": ["bug"],
                    "content": {
                        "type": "Issue",
                        "number": 42,
                        "title": "Fix login",
                        "body": "Details here",
                        "url": "https://github.com/octo-org/rusty/issues/42"
                    }
                },
                {
                    "status": "In Progress",
                    "labels": ["feature"],
                    "content": {
                        "type": "Issue",
                        "number": 99,
                        "title": "Add search",
                        "body": "Search feature",
                        "url": "https://github.com/octo-org/rusty/issues/99"
                    }
                }
            ]
        });

        let config = tracker_config();
        let issues = Adapter::parse_project_items(&json, &config).unwrap();

        assert_eq!(issues.len(), 2);

        assert_eq!(issues[0].id, "42");
        assert_eq!(issues[0].identifier, "rusty-42");
        assert_eq!(issues[0].title, "Fix login");
        assert_eq!(issues[0].description.as_deref(), Some("Details here"));
        assert_eq!(issues[0].state, "Todo");
        assert_eq!(
            issues[0].url.as_deref(),
            Some("https://github.com/octo-org/rusty/issues/42")
        );
        assert_eq!(issues[0].labels, vec!["bug"]);

        assert_eq!(issues[1].id, "99");
        assert_eq!(issues[1].identifier, "rusty-99");
        assert_eq!(issues[1].title, "Add search");
        assert_eq!(issues[1].state, "In Progress");
    }

    #[test]
    fn parse_project_items_skips_non_issue_types() {
        let json = json!({
            "items": [
                {
                    "status": "Todo",
                    "labels": [],
                    "content": {
                        "type": "PullRequest",
                        "number": 10,
                        "title": "PR title",
                        "body": "",
                        "url": "https://github.com/octo-org/rusty/pull/10"
                    }
                },
                {
                    "status": "Draft",
                    "labels": [],
                    "content": {
                        "type": "DraftIssue",
                        "number": 11,
                        "title": "Draft title",
                        "body": "",
                        "url": ""
                    }
                },
                {
                    "status": "Todo",
                    "labels": [],
                    "content": {
                        "type": "Issue",
                        "number": 1,
                        "title": "Real issue",
                        "body": "",
                        "url": "https://github.com/octo-org/rusty/issues/1"
                    }
                }
            ]
        });

        let config = tracker_config();
        let issues = Adapter::parse_project_items(&json, &config).unwrap();

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].title, "Real issue");
    }

    #[test]
    fn parse_project_items_skips_items_with_zero_number() {
        let json = json!({
            "items": [
                {
                    "status": "Todo",
                    "labels": [],
                    "content": {
                        "type": "Issue",
                        "number": 0,
                        "title": "Bad issue",
                        "body": "",
                        "url": ""
                    }
                }
            ]
        });

        let config = tracker_config();
        let issues = Adapter::parse_project_items(&json, &config).unwrap();

        assert!(issues.is_empty());
    }

    #[test]
    fn parse_project_items_extracts_priority_from_labels() {
        let json = json!({
            "items": [
                {
                    "status": "Todo",
                    "labels": ["bug", "priority-2"],
                    "content": {
                        "type": "Issue",
                        "number": 42,
                        "title": "Fix login",
                        "body": "Details here",
                        "url": "https://github.com/octo-org/rusty/issues/42"
                    }
                }
            ]
        });

        let config = tracker_config();
        let issues = Adapter::parse_project_items(&json, &config).unwrap();

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].priority, Some(2));
    }

    #[test]
    fn parse_project_items_returns_error_on_missing_items_array() {
        let json = json!({});

        let config = tracker_config();
        let result = Adapter::parse_project_items(&json, &config);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, TrackerError::UnknownPayload(_)),
            "expected UnknownPayload, got: {err:?}"
        );
    }

    #[test]
    fn parse_project_items_handles_empty_items_array() {
        let json = json!({"items": []});

        let config = tracker_config();
        let issues = Adapter::parse_project_items(&json, &config).unwrap();

        assert!(issues.is_empty());
    }

    // ── filter_project_items tests ─────────────────────────────────────

    #[test]
    fn filter_project_items_returns_all_when_states_empty() {
        let items = vec![
            test_issue("1", "rusty-1", "A", "Todo", None),
            test_issue("2", "rusty-2", "B", "Done", None),
        ];

        let result = Adapter::filter_project_items(&items, &[]);

        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_project_items_filters_by_requested_states() {
        let items = vec![
            test_issue("1", "rusty-1", "A", "Todo", None),
            test_issue("2", "rusty-2", "B", "In Progress", None),
            test_issue("3", "rusty-3", "C", "Done", None),
        ];

        // Case-insensitive: "todo" should match "Todo"
        let states = vec!["todo".to_string(), "done".to_string()];
        let result = Adapter::filter_project_items(&items, &states);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, "1");
        assert_eq!(result[1].id, "3");
    }

    // ── is_graphql_rate_limited tests ──────────────────────────────────

    #[test]
    fn is_graphql_rate_limited_detects_rate_limit() {
        assert!(Adapter::is_graphql_rate_limited(
            "API rate limit exceeded for user"
        ));
    }

    #[test]
    fn is_graphql_rate_limited_detects_secondary_rate_limit() {
        assert!(Adapter::is_graphql_rate_limited(
            "You have exceeded a secondary rate limit"
        ));
    }

    #[test]
    fn is_graphql_rate_limited_returns_false_for_other_errors() {
        assert!(!Adapter::is_graphql_rate_limited("permission denied"));
    }

    // ── extract_rate_limit_reset tests ─────────────────────────────────

    #[test]
    fn extract_rate_limit_reset_parses_unix_timestamp() {
        let stderr = "some header\nx-ratelimit-reset: 1700000000\nother stuff";
        let result = Adapter::extract_rate_limit_reset(stderr);

        assert!(result.is_some());
        let dt = result.unwrap();
        assert_eq!(dt, DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap());
    }

    #[test]
    fn extract_rate_limit_reset_returns_none_on_garbage() {
        assert!(Adapter::extract_rate_limit_reset("random garbage text").is_none());
    }

    // ── change_detection_url tests ─────────────────────────────────────

    #[test]
    fn change_detection_url_returns_error_when_repo_missing() {
        let config = TrackerConfig::default(); // no owner/repo
        let result = Adapter::change_detection_url(&config);

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), TrackerError::MissingRepo),
            "expected MissingRepo error"
        );
    }

    // ── cache_is_stale tests ───────────────────────────────────────────

    #[test]
    fn cache_is_stale_returns_true_when_no_prior_fetch() {
        let adapter = Adapter::new(tracker_config());
        assert!(adapter.cache_is_stale());
    }

    #[test]
    fn cache_is_stale_returns_false_after_recent_update() {
        let adapter = Adapter::new(tracker_config());
        *adapter.last_graphql_fetch.write().unwrap() = Some(Utc::now());
        assert!(!adapter.cache_is_stale());
    }
}
