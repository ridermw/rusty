use std::fs;

use symphony::workspace::{
    sanitize_workspace_key, verify_containment, workspace_path, WorkspaceError,
};
use tempfile::tempdir;

#[test]
fn sanitize_workspace_key_keeps_safe_identifiers() {
    assert_eq!(sanitize_workspace_key("rusty-42"), "rusty-42");
}

#[test]
fn sanitize_workspace_key_replaces_path_separator() {
    assert_eq!(sanitize_workspace_key("ABC/123"), "ABC_123");
}

#[test]
fn sanitize_workspace_key_replaces_spaces_and_punctuation() {
    assert_eq!(sanitize_workspace_key("hello world!"), "hello_world_");
}

#[test]
fn sanitize_workspace_key_replaces_leading_symbol() {
    assert_eq!(sanitize_workspace_key("#42"), "_42");
}

#[test]
fn workspace_path_joins_root_with_sanitized_key() {
    let root = tempdir().expect("create temp dir");
    let expected = root.path().join("rusty-42");

    assert_eq!(workspace_path(root.path(), "rusty-42"), expected);
}

#[test]
fn verify_containment_passes_for_path_inside_root() {
    let root = tempdir().expect("create temp dir");
    let workspace = root.path().join("rusty-42");
    fs::create_dir_all(&workspace).expect("create workspace dir");

    let result = verify_containment(&workspace, root.path());

    assert!(result.is_ok());
}

#[test]
fn verify_containment_fails_for_traversal_outside_root() {
    let root = tempdir().expect("create temp dir");
    let workspace = root.path().join("..").join("outside");

    let result = verify_containment(&workspace, root.path());

    assert!(matches!(
        result,
        Err(WorkspaceError::PathOutsideRoot { .. })
    ));
}

#[test]
fn verify_containment_fails_when_existing_workspace_resolves_outside_root() {
    let root = tempdir().expect("create temp dir");
    let nested = root.path().join("nested");
    fs::create_dir_all(&nested).expect("create nested dir");

    let outside_name = format!(
        "escaped-{}",
        root.path()
            .file_name()
            .expect("tempdir file name")
            .to_string_lossy()
    );
    let outside = root
        .path()
        .parent()
        .expect("tempdir parent")
        .join(&outside_name);
    fs::create_dir_all(&outside).expect("create outside dir");

    let workspace = nested.join("..").join("..").join(&outside_name);
    let result = verify_containment(&workspace, root.path());

    assert!(matches!(
        result,
        Err(WorkspaceError::PathOutsideRoot { .. })
    ));

    fs::remove_dir_all(&outside).expect("remove outside dir");
}
