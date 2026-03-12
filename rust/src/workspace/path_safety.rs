use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace creation failed at {0}: {1}")]
    CreationFailed(PathBuf, std::io::Error),
    #[error("workspace path {path} is outside root {root}")]
    PathOutsideRoot { path: PathBuf, root: PathBuf },
    #[error("hook '{hook}' failed with exit code {exit_code}")]
    HookFailed { hook: String, exit_code: i32 },
    #[error("hook '{hook}' timed out")]
    HookTimeout { hook: String },
    #[error("invalid workspace key: {0}")]
    InvalidKey(String),
}

/// Sanitize an issue identifier to a workspace-safe directory name.
/// Only [A-Za-z0-9._-] are allowed; everything else becomes '_'.
pub fn sanitize_workspace_key(identifier: &str) -> String {
    identifier
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Compute the workspace path for an issue under the given root.
pub fn workspace_path(root: &Path, identifier: &str) -> PathBuf {
    let key = sanitize_workspace_key(identifier);
    root.join(key)
}

/// Canonicalize a path, resolving symlinks.
/// Returns the absolute, canonical path.
pub fn canonicalize_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    #[cfg(windows)]
    {
        let canonical = std::fs::canonicalize(path)?;
        let s = canonical.to_string_lossy();

        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            Ok(PathBuf::from(stripped))
        } else {
            Ok(canonical)
        }
    }

    #[cfg(not(windows))]
    {
        std::fs::canonicalize(path)
    }
}

/// Verify that a workspace path is contained within the workspace root.
/// Both paths are canonicalized before comparison.
pub fn verify_containment(workspace: &Path, root: &Path) -> Result<(), WorkspaceError> {
    let canonical_root = canonicalize_path(root).map_err(|_| WorkspaceError::PathOutsideRoot {
        path: workspace.to_path_buf(),
        root: root.to_path_buf(),
    })?;

    let canonical_workspace = if workspace.exists() {
        canonicalize_path(workspace).map_err(|_| WorkspaceError::PathOutsideRoot {
            path: workspace.to_path_buf(),
            root: root.to_path_buf(),
        })?
    } else {
        let parent = workspace.parent().unwrap_or(Path::new("."));
        let name = workspace
            .file_name()
            .ok_or_else(|| WorkspaceError::PathOutsideRoot {
                path: workspace.to_path_buf(),
                root: root.to_path_buf(),
            })?;
        let canonical_parent =
            canonicalize_path(parent).map_err(|_| WorkspaceError::PathOutsideRoot {
                path: workspace.to_path_buf(),
                root: root.to_path_buf(),
            })?;

        canonical_parent.join(name)
    };

    if canonical_workspace.starts_with(&canonical_root) {
        Ok(())
    } else {
        Err(WorkspaceError::PathOutsideRoot {
            path: canonical_workspace,
            root: canonical_root,
        })
    }
}
