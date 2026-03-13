use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use super::{Issue, Tracker, TrackerError};
use crate::config::schema::TrackerConfig;

#[derive(Debug, Clone)]
pub struct MemoryTracker {
    issues: Arc<RwLock<Vec<Issue>>>,
}

impl MemoryTracker {
    pub fn new(issues: Vec<Issue>) -> Self {
        Self {
            issues: Arc::new(RwLock::new(issues)),
        }
    }

    pub fn set_issues(&self, issues: Vec<Issue>) {
        *self.issues.write().unwrap() = issues;
    }

    pub fn update_issue_state(&self, id: &str, new_state: &str) {
        let mut issues = self.issues.write().unwrap();
        if let Some(issue) = issues.iter_mut().find(|issue| issue.id == id) {
            issue.state = new_state.to_string();
        }
    }
}

pub fn test_issue(
    id: &str,
    identifier: &str,
    title: &str,
    state: &str,
    priority: Option<i32>,
) -> Issue {
    Issue {
        id: id.to_string(),
        identifier: identifier.to_string(),
        title: title.to_string(),
        description: None,
        priority,
        state: state.to_string(),
        branch_name: None,
        url: None,
        labels: vec![],
        blocked_by: vec![],
        created_at: None,
        updated_at: None,
    }
}

#[async_trait]
impl Tracker for MemoryTracker {
    async fn fetch_candidate_issues(
        &self,
        config: &TrackerConfig,
    ) -> Result<Vec<Issue>, TrackerError> {
        let issues = self.issues.read().unwrap();
        let active: Vec<String> = config
            .active_states
            .iter()
            .map(|state| state.to_lowercase())
            .collect();

        let mut candidates: Vec<Issue> = issues
            .iter()
            .filter(|issue| active.contains(&issue.state.to_lowercase()))
            .cloned()
            .collect();

        if !config.active_issue_labels.is_empty() {
            let required: Vec<String> = config
                .active_issue_labels
                .iter()
                .map(|label| label.to_lowercase())
                .collect();
            candidates.retain(|issue| {
                issue
                    .labels
                    .iter()
                    .any(|label| required.contains(&label.to_lowercase()))
            });
        }

        Ok(candidates)
    }

    async fn fetch_issue_states_by_ids(&self, ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        let issues = self.issues.read().unwrap();
        Ok(issues
            .iter()
            .filter(|issue| ids.contains(&issue.id))
            .cloned()
            .collect())
    }

    async fn fetch_issues_by_states(
        &self,
        states: &[String],
        _config: &TrackerConfig,
    ) -> Result<Vec<Issue>, TrackerError> {
        let issues = self.issues.read().unwrap();
        let normalized: Vec<String> = states.iter().map(|state| state.to_lowercase()).collect();

        Ok(issues
            .iter()
            .filter(|issue| normalized.contains(&issue.state.to_lowercase()))
            .cloned()
            .collect())
    }
}
