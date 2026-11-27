use super::Recorder;

#[tokio::test(start_paused = true)]
async fn waiting_for_what_never_arrives_returns_what_is_there() {
    let recorder = Recorder::default();
    assert!(recorder.logging(1).await.is_empty());
    assert!(recorder.progress(1).await.is_empty());
}
