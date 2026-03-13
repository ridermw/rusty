use rusty::logging::init_logging;

#[test]
fn init_logging_with_log_dir_creates_directory_and_guard() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let log_dir = temp_dir.path().join("logs");

    let guard = init_logging(Some(log_dir.as_path())).expect("initialize logging");

    assert!(log_dir.exists());
    assert!(guard.is_some());
}
