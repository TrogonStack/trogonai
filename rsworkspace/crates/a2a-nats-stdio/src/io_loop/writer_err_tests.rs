use super::writer_task_err;

#[test]
fn writer_task_err_surfaces_io_error_from_task() {
    let io_err = std::io::Error::other("write boom");
    let mapped = writer_task_err(Ok(Err(io_err)));
    assert_eq!(mapped.to_string(), "write boom");
}

#[test]
fn writer_task_err_maps_clean_exit_to_unexpected() {
    let err = writer_task_err(Ok(Ok(())));
    assert!(err.to_string().contains("writer task exited unexpectedly"));
}

#[tokio::test]
async fn writer_task_err_maps_join_failure() {
    let handle = tokio::spawn(async { panic!("writer panicked") });
    let join_err = handle.await.unwrap_err();
    let err = writer_task_err(Err(join_err));
    assert!(err.to_string().contains("writer panicked"));
}
