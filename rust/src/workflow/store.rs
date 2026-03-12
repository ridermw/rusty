use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::{load_workflow, WorkflowDefinition};

pub struct WorkflowStore {
    path: PathBuf,
    current: Arc<RwLock<WorkflowDefinition>>,
    _watcher: notify::RecommendedWatcher,
}

impl WorkflowStore {
    pub fn new(
        path: &Path,
        reload_tx: mpsc::Sender<WorkflowDefinition>,
    ) -> Result<Self, crate::config::ConfigError> {
        let workflow = load_workflow(path)?;
        let current = Arc::new(RwLock::new(workflow));

        let watch_path = path.to_path_buf();
        let watch_current = Arc::clone(&current);
        let watch_tx = reload_tx;

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) => {
                    match load_workflow(&watch_path) {
                        Ok(new_workflow) => {
                            info!(path = %watch_path.display(), "WORKFLOW.md reloaded successfully");
                            *watch_current.write().unwrap() = new_workflow.clone();

                            if let Err(send_error) = watch_tx.blocking_send(new_workflow) {
                                warn!(path = %watch_path.display(), error = %send_error, "failed to send workflow reload notification");
                            }
                        }
                        Err(err) => {
                            warn!(path = %watch_path.display(), error = %err, "WORKFLOW.md reload failed, keeping last-known-good");
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    error!(path = %watch_path.display(), error = %err, "WORKFLOW.md watcher error");
                }
            }
        })
        .map_err(|e| {
            crate::config::ConfigError::WorkflowParseError(format!(
                "failed to create file watcher: {e}"
            ))
        })?;

        watcher
            .watch(path, RecursiveMode::NonRecursive)
            .map_err(|e| {
                crate::config::ConfigError::WorkflowParseError(format!("failed to watch path: {e}"))
            })?;

        Ok(Self {
            path: path.to_path_buf(),
            current,
            _watcher: watcher,
        })
    }

    pub fn current(&self) -> WorkflowDefinition {
        self.current.read().unwrap().clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
