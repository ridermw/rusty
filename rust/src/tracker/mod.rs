pub mod github;
pub mod memory;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::schema::TrackerConfig;

#[derive(Debug, Error)]
pub enum TrackerError {
    #[error("unsupported tracker kind: {0}")]
    UnsupportedKind(String),
    #[error("missing tracker API key")]
    MissingApiKey,
    #[error("missing tracker repo")]
    MissingRepo,
    #[error("API request failed: {0}")]
    ApiRequest(String),
    #[error("API returned status {0}: {1}")]
    ApiStatus(u16, String),
    #[error("GraphQL errors: {0:?}")]
    GraphqlErrors(Vec<serde_json::Value>),
    #[error("rate limited until {reset_at}")]
    RateLimited { reset_at: DateTime<Utc> },
    #[error("unknown payload: {0}")]
    UnknownPayload(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Issue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<i32>,
    pub state: String,
    pub branch_name: Option<String>,
    pub url: Option<String>,
    pub labels: Vec<String>,
    pub blocked_by: Vec<BlockerRef>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockerRef {
    pub id: Option<String>,
    pub identifier: Option<String>,
    pub state: Option<String>,
}

#[async_trait]
pub trait Tracker: Send + Sync {
    /// Fetch candidate issues in active states for the configured project.
    async fn fetch_candidate_issues(
        &self,
        config: &TrackerConfig,
    ) -> Result<Vec<Issue>, TrackerError>;

    /// Fetch current states for specific issue IDs (reconciliation).
    async fn fetch_issue_states_by_ids(&self, ids: &[String]) -> Result<Vec<Issue>, TrackerError>;

    /// Fetch issues in specified states (e.g., terminal states for startup cleanup).
    async fn fetch_issues_by_states(
        &self,
        states: &[String],
        config: &TrackerConfig,
    ) -> Result<Vec<Issue>, TrackerError>;

    /// Persist a session ID for the given issue (e.g. as a hidden comment).
    async fn save_session_id(&self, issue_id: &str, session_id: &str) -> Result<(), TrackerError>;

    /// Load the previously saved session ID for the given issue.
    async fn load_session_id(&self, issue_id: &str) -> Result<Option<String>, TrackerError>;

    /// Delete the saved session ID for the given issue (cleanup).
    async fn delete_session_id(&self, issue_id: &str) -> Result<(), TrackerError>;
}
