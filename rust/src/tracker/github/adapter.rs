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
}

#[async_trait]
impl Tracker for GitHubAdapter {
    async fn fetch_candidate_issues(
        &self,
        config: &TrackerConfig,
    ) -> Result<Vec<Issue>, TrackerError> {
        let labels = if config.labels.is_empty() {
            None
        } else {
            Some(config.labels.as_slice())
        };

        self.client.fetch_issues(config, "open", labels).await
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
        let github_state = if states
            .iter()
            .any(|state| state.eq_ignore_ascii_case("closed"))
        {
            "closed"
        } else {
            "open"
        };

        let all = self.client.fetch_issues(config, github_state, None).await?;

        // Post-filter to only include issues whose resolved state matches the
        // exact requested states (not the entire open/closed bucket).
        let requested: Vec<String> = states.iter().map(|s| s.to_lowercase()).collect();
        Ok(all
            .into_iter()
            .filter(|issue| requested.contains(&issue.state.to_lowercase()))
            .collect())
    }
}
