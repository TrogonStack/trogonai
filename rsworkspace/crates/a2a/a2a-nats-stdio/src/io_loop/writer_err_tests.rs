use super::writer_task_result;
use std::io::ErrorKind;

#[test]
fn writer_task_result_surfaces_io_error_from_task() {
    let io_err = std::io::Error::new(ErrorKind::BrokenPipe, "write boom");
    let mapped = writer_task_result(Ok(Err(io_err))).expect_err("writer IO failure is preserved");
    assert_eq!(mapped.kind(), ErrorKind::BrokenPipe);
}

#[test]
fn writer_task_result_preserves_clean_completion() {
    assert!(writer_task_result(Ok(Ok(()))).is_ok());
}

#[tokio::test]
async fn writer_task_result_maps_join_failure() {
    let handle = tokio::spawn(async { panic!("writer panicked") });
    let join_err = handle.await.unwrap_err();
    let mapped = writer_task_result(Err(join_err)).expect_err("writer panic is preserved");
    let inner = mapped.into_inner().expect("join error wrapped as inner source");
    let join = inner
        .downcast::<tokio::task::JoinError>()
        .expect("inner source is the original JoinError");
    assert!(join.is_panic());
}
