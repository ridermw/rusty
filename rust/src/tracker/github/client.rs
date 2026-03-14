use std::collections::HashMap;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use tracing::{debug, warn};

use crate::config::schema::TrackerConfig;
use crate::ports::{HttpClient, ReqwestHttpClient};
use crate::tracker::{Issue, TrackerError};

pub struct GitHubClient<H: HttpClient = ReqwestHttpClient> {
    http: H,
    etag_cache: RwLock<HashMap<String, String>>,
    /// Cached issue payloads from last successful fetch, keyed by base URL+state.
    /// Used to return previous results on 304 Not Modified responses.
    response_cache: RwLock<HashMap<String, Vec<Issue>>>,
}

impl GitHubClient {
    pub fn new() -> Self {
        Self {
            http: ReqwestHttpClient::new(),
            etag_cache: RwLock::new(HashMap::new()),
            response_cache: RwLock::new(HashMap::new()),
        }
    }
}

impl<H: HttpClient> GitHubClient<H> {
    pub fn with_http(http: H) -> Self {
        Self {
            http,
            etag_cache: RwLock::new(HashMap::new()),
            response_cache: RwLock::new(HashMap::new()),
        }
    }

    async fn resolve_token(config: &TrackerConfig) -> Result<String, TrackerError> {
        crate::config::resolve_github_token(config.api_key.as_deref())
            .await
            .map_err(|_| TrackerError::MissingApiKey)
    }

    fn issues_url(config: &TrackerConfig) -> Result<String, TrackerError> {
        let repo = config.full_repo().ok_or(TrackerError::MissingRepo)?;
        let endpoint = config
            .endpoint
            .as_deref()
            .unwrap_or("https://api.github.com")
            .trim_end_matches('/');
        Ok(format!("{endpoint}/repos/{repo}/issues"))
    }

    fn repo_name(config: &TrackerConfig) -> String {
        config
            .full_repo()
            .as_deref()
            .and_then(|repo| repo.split('/').next_back())
            .unwrap_or("repo")
            .to_string()
    }

    pub async fn fetch_issues(
        &self,
        config: &TrackerConfig,
        state: &str,
        labels: Option<&[String]>,
    ) -> Result<Vec<Issue>, TrackerError> {
        let token = Self::resolve_token(config).await?;
        let base_url = Self::issues_url(config)?;
        let repo_name = Self::repo_name(config);
        let cache_key = format!("{base_url}?state={state}");
        let mut all_issues = Vec::new();
        let mut page = 1_u32;
        let bearer = format!("Bearer {token}");

        loop {
            let mut url = format!("{base_url}?state={state}&per_page=50&page={page}");
            if let Some(labels) = labels.filter(|labels| !labels.is_empty()) {
                url.push_str(&format!("&labels={}", labels.join(",")));
            }

            debug!(%url, page, "fetching GitHub issues page");

            let mut headers = vec![
                ("Authorization", bearer.as_str()),
                ("Accept", "application/vnd.github+json"),
                ("User-Agent", "rusty/0.1"),
            ];

            let etag_value = self.etag_cache.read().unwrap().get(&url).cloned();
            if let Some(ref etag) = etag_value {
                headers.push(("If-None-Match", etag.as_str()));
            }

            let response = self
                .http
                .get(&url, &headers)
                .await
                .map_err(|err| TrackerError::ApiRequest(err.to_string()))?;
            let status = response.status;

            if let Some(etag_str) = response.header("etag") {
                self.etag_cache
                    .write()
                    .unwrap()
                    .insert(url.clone(), etag_str.to_string());
            }

            match status {
                304 => {
                    debug!(%url, "GitHub API returned 304 Not Modified");
                    if let Some(cached) = self.response_cache.read().unwrap().get(&cache_key) {
                        return Ok(cached.clone());
                    }
                    break;
                }
                429 => {
                    let reset_at = response
                        .header("x-ratelimit-reset")
                        .and_then(|value| value.parse::<i64>().ok())
                        .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
                        .unwrap_or_else(Utc::now);
                    warn!(%url, ?reset_at, "GitHub API rate limited request");
                    return Err(TrackerError::RateLimited { reset_at });
                }
                200 => {}
                _ => {
                    let body = response.text();
                    return Err(TrackerError::ApiStatus(status, body));
                }
            }

            let items: Vec<serde_json::Value> = response
                .json()
                .map_err(|err| TrackerError::UnknownPayload(err.to_string()))?;

            if items.is_empty() {
                break;
            }

            for item in &items {
                if let Some(issue) = normalize_github_issue(item, &repo_name, config) {
                    all_issues.push(issue);
                }
            }

            if items.len() < 50 {
                break;
            }

            page += 1;
        }

        // Cache successful response for 304 handling
        self.response_cache
            .write()
            .unwrap()
            .insert(cache_key, all_issues.clone());

        Ok(all_issues)
    }

    pub async fn fetch_issues_by_numbers(
        &self,
        config: &TrackerConfig,
        numbers: &[u64],
    ) -> Result<Vec<Issue>, TrackerError> {
        let token = Self::resolve_token(config).await?;
        let repo = config.full_repo().ok_or(TrackerError::MissingRepo)?;
        let endpoint = config
            .endpoint
            .as_deref()
            .unwrap_or("https://api.github.com")
            .trim_end_matches('/');
        let repo_name = Self::repo_name(config);
        let mut issues = Vec::new();
        let bearer = format!("Bearer {token}");

        for &number in numbers {
            let url = format!("{endpoint}/repos/{repo}/issues/{number}");
            let headers = [
                ("Authorization", bearer.as_str()),
                ("Accept", "application/vnd.github+json"),
                ("User-Agent", "rusty/0.1"),
            ];
            let response = self
                .http
                .get(&url, &headers)
                .await
                .map_err(|err| TrackerError::ApiRequest(err.to_string()))?;

            let status = response.status;
            match status {
                200 => {
                    let item: serde_json::Value = response
                        .json()
                        .map_err(|err| TrackerError::UnknownPayload(err.to_string()))?;
                    if let Some(issue) = normalize_github_issue(&item, &repo_name, config) {
                        issues.push(issue);
                    }
                }
                429 => {
                    let reset_at = response
                        .header("x-ratelimit-reset")
                        .and_then(|value| value.parse::<i64>().ok())
                        .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
                        .unwrap_or_else(Utc::now);
                    warn!(%url, ?reset_at, "GitHub API rate limited request");
                    return Err(TrackerError::RateLimited { reset_at });
                }
                _ => {
                    warn!(%url, status, "GitHub issue fetch returned non-success status");
                }
            }
        }

        Ok(issues)
    }
}

impl Default for GitHubClient {
    fn default() -> Self {
        Self::new()
    }
}

pub fn normalize_github_issue(
    item: &serde_json::Value,
    repo_name: &str,
    config: &TrackerConfig,
) -> Option<Issue> {
    if item.get("pull_request").is_some() {
        return None;
    }

    let number = item.get("number")?.as_u64()?;
    let title = item.get("title")?.as_str()?.to_string();
    let github_state = item.get("state")?.as_str()?.to_string();

    let labels: Vec<String> = item
        .get("labels")
        .and_then(|labels| labels.as_array())
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.get("name").and_then(|name| name.as_str()))
                .map(|label| label.to_lowercase())
                .collect()
        })
        .unwrap_or_default();

    let state = if config.state_labels.is_empty() {
        github_state.clone()
    } else {
        labels
            .iter()
            .find_map(|label| config.state_labels.get(label).cloned())
            .unwrap_or_else(|| github_state.clone())
    };

    let priority = labels.iter().find_map(|label| {
        label
            .strip_prefix("priority-")
            .and_then(|value| value.parse::<i32>().ok())
    });

    Some(Issue {
        id: number.to_string(),
        identifier: format!("{repo_name}-{number}"),
        title,
        description: item
            .get("body")
            .and_then(|body| body.as_str())
            .map(str::to_string),
        priority,
        state,
        branch_name: None,
        url: item
            .get("html_url")
            .and_then(|url| url.as_str())
            .map(str::to_string),
        labels,
        blocked_by: Vec::new(),
        created_at: item
            .get("created_at")
            .and_then(|date| date.as_str())
            .and_then(|date| date.parse::<DateTime<Utc>>().ok()),
        updated_at: item
            .get("updated_at")
            .and_then(|date| date.as_str())
            .and_then(|date| date.parse::<DateTime<Utc>>().ok()),
    })
}
