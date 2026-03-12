pub mod hooks;
pub mod path_safety;

pub use path_safety::{sanitize_workspace_key, verify_containment, workspace_path, WorkspaceError};

use std::path::{Path, PathBuf};

use tracing::{info, warn};

/// Result of preparing a workspace for an issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub path: PathBuf,
    pub workspace_key: String,
    pub created_now: bool,
}

/// Create or reuse a workspace for an issue.
pub fn create_for_issue(root: &Path, identifier: &str) -> Result<Workspace, WorkspaceError> {
    let key = sanitize_workspace_key(identifier);
    let ws_path = workspace_path(root, identifier);

    std::fs::create_dir_all(root)
        .map_err(|e| WorkspaceError::CreationFailed(root.to_path_buf(), e))?;

    verify_containment(&ws_path, root)?;

    let created_now = if ws_path.exists() {
        if ws_path.is_dir() {
            info!(workspace = %ws_path.display(), "reusing existing workspace");
            false
        } else {
            warn!(workspace = %ws_path.display(), "workspace path existed as a file; recreating directory");
            std::fs::remove_file(&ws_path)
                .map_err(|e| WorkspaceError::CreationFailed(ws_path.clone(), e))?;
            std::fs::create_dir_all(&ws_path)
                .map_err(|e| WorkspaceError::CreationFailed(ws_path.clone(), e))?;
            true
        }
    } else {
        std::fs::create_dir_all(&ws_path)
            .map_err(|e| WorkspaceError::CreationFailed(ws_path.clone(), e))?;
        info!(workspace = %ws_path.display(), "created new workspace");
        true
    };

    Ok(Workspace {
        path: ws_path,
        workspace_key: key,
        created_now,
    })
}

/// Remove a workspace directory.
pub fn remove_workspace(root: &Path, identifier: &str) -> Result<(), WorkspaceError> {
    let ws_path = workspace_path(root, identifier);
    if !ws_path.exists() {
        return Ok(());
    }

    verify_containment(&ws_path, root)?;

    if ws_path.is_dir() {
        std::fs::remove_dir_all(&ws_path)
            .map_err(|e| WorkspaceError::CreationFailed(ws_path.clone(), e))?;
    } else {
        std::fs::remove_file(&ws_path)
            .map_err(|e| WorkspaceError::CreationFailed(ws_path.clone(), e))?;
    }

    info!(workspace = %ws_path.display(), "removed workspace");
    Ok(())
}
