use grpc_nats_micro::answer;
use grpc_nats_micro::status_codec::decode_reply;
use trogonai_proto::nats::micro::v1alpha1::{ContentType as ProtoContentType, ServiceOptions};
use trogonai_proto::secrets::exchange::v1alpha1::resolve_for_release_response::Result as Outcome;
use trogonai_proto::secrets::exchange::v1alpha1::{ResolveFailureCategory, ResolveForReleaseResponse};

use super::*;
use crate::THROTTLED_RETRY_AFTER;

#[tokio::test]
async fn any_payload_is_throttled_with_the_throttle_delay() {
    let options = ServiceOptions {
        content_type: ProtoContentType::CONTENT_TYPE_PROTOBUF.into(),
        ..Default::default()
    };
    let reply = answer(None, &[0xff, 0xff, 0xff], &options, &ThrottledAnswer)
        .await
        .unwrap();
    let response =
        decode_reply::<ResolveForReleaseResponse>(Some(&reply.headers), &reply.body, ContentType::Protobuf).unwrap();
    let Some(Outcome::Failure(failure)) = response.result else {
        panic!("a throttled call answered an access");
    };
    assert_eq!(
        failure.category.as_known(),
        Some(ResolveFailureCategory::RESOLVE_FAILURE_CATEGORY_THROTTLED)
    );
    assert_eq!(
        failure.retry_after.as_option(),
        Some(&buffa_types::google::protobuf::Duration::from(THROTTLED_RETRY_AFTER))
    );
}
