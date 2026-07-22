use super::*;

#[tokio::test]
async fn noop_channel_reports_unavailable() {
    let channel = NoopInteractionChannel;
    let notice = InteractionNotice {
        url: "https://ps.example/interact/abc".to_string(),
        code: "ABC-123".to_string(),
        description: Some("confirm payment".to_string()),
    };
    let err = channel.notify(&notice).await.unwrap_err();
    assert!(matches!(err, InteractionRelayError::Unavailable));
}
