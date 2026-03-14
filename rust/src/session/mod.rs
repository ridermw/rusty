//! Session persistence – save and resume ACP sessions across restarts.
//!
//! Sessions are stored in a `sessions.json` file in the workspace root
//! directory.  Each record maps an issue ID to the ACP session ID that
//! was active when the agent last ran for that issue.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// One persisted session record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRecord {
    pub issue_id: String,
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub workspace_path: Option<String>,
}

/// In-memory session store backed by a `sessions.json` file.
#[derive(Debug)]
pub struct SessionStore {
    path: PathBuf,
}

const SESSIONS_FILE: &str = "sessions.json";
const STALE_HOURS: i64 = 24;

impl SessionStore {
    /// Create a store rooted at `workspace_root`.
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            path: workspace_root.join(SESSIONS_FILE),
        }
    }

    /// Path to the backing JSON file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    // ── read / write helpers ──────────────────────────────────────────

    fn read_all(&self) -> HashMap<String, SessionRecord> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    fn write_all(&self, records: &HashMap<String, SessionRecord>) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(records)
            .map_err(std::io::Error::other)?;
        std::fs::write(&self.path, json)
    }

    // ── public API ────────────────────────────────────────────────────

    /// Persist a session record for `issue_id`.
    pub fn save(&self, record: SessionRecord) -> std::io::Result<()> {
        let mut records = self.read_all();
        let issue_id = record.issue_id.clone();
        debug!(%issue_id, session_id = %record.session_id, "saving session record");
        records.insert(issue_id, record);
        self.write_all(&records)
    }

    /// Load the session record for `issue_id`, if any.
    pub fn load(&self, issue_id: &str) -> Option<SessionRecord> {
        self.read_all().remove(issue_id)
    }

    /// Delete the session record for `issue_id`.
    pub fn delete(&self, issue_id: &str) -> std::io::Result<()> {
        let mut records = self.read_all();
        if records.remove(issue_id).is_some() {
            debug!(%issue_id, "deleted session record");
            self.write_all(&records)?;
        }
        Ok(())
    }

    /// List all stored session records.
    pub fn list(&self) -> Vec<SessionRecord> {
        self.read_all().into_values().collect()
    }

    /// Remove records older than 24 hours.
    pub fn cleanup_stale(&self) -> std::io::Result<usize> {
        let mut records = self.read_all();
        let cutoff = Utc::now() - Duration::hours(STALE_HOURS);
        let before = records.len();
        records.retain(|_id, record| record.created_at > cutoff);
        let removed = before - records.len();
        if removed > 0 {
            info!(removed, "cleaned up stale session records");
            self.write_all(&records)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_record(issue_id: &str, session_id: &str) -> SessionRecord {
        SessionRecord {
            issue_id: issue_id.to_string(),
            session_id: session_id.to_string(),
            created_at: Utc::now(),
            workspace_path: Some("/tmp/ws".to_string()),
        }
    }

    #[test]
    fn save_and_load_round_trips() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());

        store.save(make_record("42", "sess-abc")).unwrap();
        let loaded = store.load("42").unwrap();

        assert_eq!(loaded.session_id, "sess-abc");
        assert_eq!(loaded.issue_id, "42");
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());

        assert!(store.load("999").is_none());
    }

    #[test]
    fn delete_removes_record() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());

        store.save(make_record("42", "sess-abc")).unwrap();
        store.delete("42").unwrap();

        assert!(store.load("42").is_none());
    }

    #[test]
    fn delete_missing_is_ok() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());
        store.delete("999").unwrap();
    }

    #[test]
    fn list_returns_all_records() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());

        store.save(make_record("1", "sess-a")).unwrap();
        store.save(make_record("2", "sess-b")).unwrap();

        let all = store.list();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn cleanup_stale_removes_old_records() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());

        let old = SessionRecord {
            issue_id: "old".to_string(),
            session_id: "sess-old".to_string(),
            created_at: Utc::now() - Duration::hours(25),
            workspace_path: None,
        };
        let fresh = make_record("fresh", "sess-fresh");

        store.save(old).unwrap();
        store.save(fresh).unwrap();

        let removed = store.cleanup_stale().unwrap();
        assert_eq!(removed, 1);
        assert!(store.load("old").is_none());
        assert!(store.load("fresh").is_some());
    }

    #[test]
    fn save_overwrites_existing_record() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());

        store.save(make_record("42", "sess-v1")).unwrap();
        store.save(make_record("42", "sess-v2")).unwrap();

        let loaded = store.load("42").unwrap();
        assert_eq!(loaded.session_id, "sess-v2");
    }

    #[test]
    fn empty_file_returns_empty() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path());

        assert!(store.list().is_empty());
        assert!(store.load("1").is_none());
    }
}
