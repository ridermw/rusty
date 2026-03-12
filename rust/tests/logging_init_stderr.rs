use symphony::logging::init_logging;

#[test]
fn init_logging_stderr_only_succeeds() {
    assert!(init_logging(None).is_ok());
}
