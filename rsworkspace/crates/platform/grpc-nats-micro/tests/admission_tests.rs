use grpc_nats_micro::{EndpointAdmission, EndpointHandler, RequestMetadata, ServerSubjectPattern};
use trogon_nats::AdvancedMockNatsClient;
use trogonai_proto::nats::micro::v1alpha1::ServiceOptions;

struct Deny;
impl EndpointAdmission for Deny {
    fn admit(&self, _: RequestMetadata<'_>) -> Option<Box<dyn EndpointHandler + '_>> {
        None
    }
}

#[tokio::test]
async fn rejected_metadata_drops_even_an_unsupported_encoding_before_any_publish() {
    let client = AdvancedMockNatsClient::new();
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Content-Type", "application/unsafe");
    let message = async_nats::Message {
        subject: "peer.v1.caller.PeerService.Send".into(),
        reply: Some("outside.reply".into()),
        payload: vec![255].into(),
        headers: Some(headers),
        status: None,
        description: None,
        length: 1,
    };
    grpc_nats_micro::server::dispatch_admitted(&client, &message, &ServiceOptions::default(), &Deny).await;
    assert!(
        client.published_messages().is_empty(),
        "admission must precede error publication"
    );
}

#[test]
fn subscriptions_accept_whole_token_stars_but_reject_malformed_patterns() {
    assert!(ServerSubjectPattern::new("peer.v1.*.PeerService.Send").is_ok());
    for invalid in [
        "",
        "flat",
        "peer..Send",
        "peer.v1.c*",
        "peer.>.Send",
        "peer.v1.>",
        "peer. v1.Send",
    ] {
        assert!(ServerSubjectPattern::new(invalid).is_err(), "accepted {invalid:?}");
    }
}
