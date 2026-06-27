use super::writer_task_err;
use std::io::ErrorKind;

#[test]
fn writer_task_err_surfaces_io_error_from_task() {
    let io_err = std::io::Error::new(ErrorKind::BrokenPipe, "write boom");
    let mapped = writer_task_err(Ok(Err(io_err)));
    assert_eq!(mapped.kind(), ErrorKind::BrokenPipe);
}

#[test]
fn writer_task_err_maps_clean_exit_to_unexpected() {
    let err = writer_task_err(Ok(Ok(())));
    assert_eq!(err.kind(), ErrorKind::Other);
}

#[tokio::test]
async fn writer_task_err_maps_join_failure() {
    let handle = tokio::spawn(async { panic!("writer panicked") });
    let join_err = handle.await.unwrap_err();
    let mapped = writer_task_err(Err(join_err));
    let inner = mapped.into_inner().expect("join error wrapped as inner source");
    let join = inner
        .downcast::<tokio::task::JoinError>()
        .expect("inner source is the original JoinError");
    assert!(join.is_panic());
}
