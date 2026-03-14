use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use super::{Issue, Tracker, TrackerError};
use crate::config::schema::TrackerConfig;

#[derive(Debug, Clone)]
pub struct MemoryTracker {
    issues: Arc<RwLock<Vec<Issue>>>,
    sessions: Arc<RwLock<HashMap<String, String>>>,
}

impl MemoryTracker {
    pub fn new(issues: Vec<Issue>) -> Self {
        Self {
            issues: Arc::new(RwLock::new(issues)),
            sessions: Arc::new(RwLock::new(HashMap::new())),
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

    async fn save_session_id(
        &self,
        issue_id: &str,
        session_id: &str,
    ) -> Result<(), TrackerError> {
        self.sessions
            .write()
            .unwrap()
            .insert(issue_id.to_string(), session_id.to_string());
        Ok(())
    }

    async fn load_session_id(&self, issue_id: &str) -> Result<Option<String>, TrackerError> {
        Ok(self.sessions.read().unwrap().get(issue_id).cloned())
    }

    async fn delete_session_id(&self, issue_id: &str) -> Result<(), TrackerError> {
        self.sessions.write().unwrap().remove(issue_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_issue_factory_produces_correct_defaults() {
        let issue = test_issue("1", "PROJ-1", "Fix bug", "open", Some(3));
        assert_eq!(issue.id, "1");
        assert_eq!(issue.identifier, "PROJ-1");
        assert_eq!(issue.title, "Fix bug");
        assert_eq!(issue.state, "open");
        assert_eq!(issue.priority, Some(3));
        assert!(issue.description.is_none());
        assert!(issue.branch_name.is_none());
        assert!(issue.url.is_none());
        assert!(issue.labels.is_empty());
        assert!(issue.blocked_by.is_empty());
        assert!(issue.created_at.is_none());
        assert!(issue.updated_at.is_none());
    }

    #[tokio::test]
    async fn new_creates_tracker_with_given_issues() {
        let issues = vec![
            test_issue("1", "P-1", "First", "open", None),
            test_issue("2", "P-2", "Second", "closed", None),
        ];
        let tracker = MemoryTracker::new(issues);

        let result = tracker
            .fetch_issue_states_by_ids(&["1".to_string(), "2".to_string()])
            .await
            .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, "1");
        assert_eq!(result[1].id, "2");
    }

    #[tokio::test]
    async fn set_issues_replaces_all_issues() {
        let tracker = MemoryTracker::new(vec![test_issue("1", "P-1", "Old", "open", None)]);
        tracker.set_issues(vec![
            test_issue("10", "P-10", "New A", "open", None),
            test_issue("11", "P-11", "New B", "closed", None),
        ]);

        let result = tracker
            .fetch_issue_states_by_ids(&["1".to_string()])
            .await
            .unwrap();
        assert!(result.is_empty(), "old issue should be gone");

        let result = tracker
            .fetch_issue_states_by_ids(&["10".to_string(), "11".to_string()])
            .await
            .unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn update_issue_state_changes_matching_issue() {
        let tracker = MemoryTracker::new(vec![
            test_issue("1", "P-1", "A", "open", None),
            test_issue("2", "P-2", "B", "open", None),
        ]);
        tracker.update_issue_state("1", "closed");

        let issues = tracker.issues.read().unwrap();
        assert_eq!(issues[0].state, "closed");
        assert_eq!(issues[1].state, "open");
    }

    #[test]
    fn update_issue_state_ignores_missing_id() {
        let tracker = MemoryTracker::new(vec![test_issue("1", "P-1", "A", "open", None)]);
        tracker.update_issue_state("999", "closed");

        let issues = tracker.issues.read().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].state, "open");
    }

    #[tokio::test]
    async fn fetch_candidate_issues_filters_by_active_states() {
        let tracker = MemoryTracker::new(vec![
            test_issue("1", "P-1", "A", "Open", None),
            test_issue("2", "P-2", "B", "closed", None),
            test_issue("3", "P-3", "C", "OPEN", None),
        ]);
        let config = TrackerConfig {
            active_states: vec!["open".to_string()],
            ..TrackerConfig::default()
        };

        let result = tracker.fetch_candidate_issues(&config).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|i| i.state.to_lowercase() == "open"));
    }

    #[tokio::test]
    async fn fetch_candidate_issues_filters_by_active_issue_labels() {
        let issues = vec![
            Issue {
                id: "1".to_string(),
                identifier: "P-1".to_string(),
                title: "With label".to_string(),
                description: None,
                priority: None,
                state: "open".to_string(),
                branch_name: None,
                url: None,
                labels: vec!["bug".to_string(), "urgent".to_string()],
                blocked_by: vec![],
                created_at: None,
                updated_at: None,
            },
            Issue {
                id: "2".to_string(),
                identifier: "P-2".to_string(),
                title: "No matching label".to_string(),
                description: None,
                priority: None,
                state: "open".to_string(),
                branch_name: None,
                url: None,
                labels: vec!["feature".to_string()],
                blocked_by: vec![],
                created_at: None,
                updated_at: None,
            },
            Issue {
                id: "3".to_string(),
                identifier: "P-3".to_string(),
                title: "Case insensitive label".to_string(),
                description: None,
                priority: None,
                state: "open".to_string(),
                branch_name: None,
                url: None,
                labels: vec!["BUG".to_string()],
                blocked_by: vec![],
                created_at: None,
                updated_at: None,
            },
        ];
        let tracker = MemoryTracker::new(issues);
        let config = TrackerConfig {
            active_states: vec!["open".to_string()],
            active_issue_labels: vec!["bug".to_string()],
            ..TrackerConfig::default()
        };

        let result = tracker.fetch_candidate_issues(&config).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|i| i.id == "1"));
        assert!(result.iter().any(|i| i.id == "3"));
    }

    #[tokio::test]
    async fn fetch_candidate_issues_returns_empty_when_no_active_states_match() {
        let tracker = MemoryTracker::new(vec![
            test_issue("1", "P-1", "A", "open", None),
            test_issue("2", "P-2", "B", "closed", None),
        ]);
        let config = TrackerConfig {
            active_states: vec!["in_progress".to_string()],
            ..TrackerConfig::default()
        };

        let result = tracker.fetch_candidate_issues(&config).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn fetch_issue_states_by_ids_returns_matching_ignores_missing() {
        let tracker = MemoryTracker::new(vec![
            test_issue("1", "P-1", "A", "open", None),
            test_issue("2", "P-2", "B", "closed", None),
        ]);

        let result = tracker
            .fetch_issue_states_by_ids(&["1".to_string(), "999".to_string()])
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "1");
    }

    #[tokio::test]
    async fn fetch_issues_by_states_case_insensitive() {
        let tracker = MemoryTracker::new(vec![
            test_issue("1", "P-1", "A", "Todo", None),
            test_issue("2", "P-2", "B", "done", None),
        ]);
        let config = TrackerConfig::default();

        let result = tracker
            .fetch_issues_by_states(&["TODO".to_string()], &config)
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "1");
    }
}
