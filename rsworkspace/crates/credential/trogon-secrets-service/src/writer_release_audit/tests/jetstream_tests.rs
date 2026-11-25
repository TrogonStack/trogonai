use std::num::NonZeroUsize;

use buffa::Message;
use trogon_audit::{AuditStream, JetStreamSink, ensure_audit_stream};
use trogon_nats::test_support::JetStreamTestServer;
use trogonai_proto::platform::audit::v1::AuditRecord;

use super::{audit_over, denial, release};
use crate::ReleaseAudit;
use crate::fixtures::{CORRELATION_ID, OWNER};

/// The release audit against a real JetStream server, with the stream
/// provisioned the way an operator provisions it.
#[tokio::test]
async fn a_release_and_a_denial_land_on_the_audit_stream_with_the_correlation_id() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    ensure_audit_stream(&js, AuditStream::Audit, NonZeroUsize::MIN)
        .await
        .unwrap();
    let (audit, _writer) = audit_over(JetStreamSink::with_defaults(js.clone()));

    audit.record_release(&release()).await.unwrap();
    audit.record_denial(&denial()).await.unwrap();

    let stream = js.get_stream("AUDIT").await.unwrap();
    assert_eq!(
        stream.cached_info().state.messages,
        3,
        "the chain reset, the release, then the denial"
    );
    for (sequence, subject) in [
        (2, format!("platform.v1.audit.credential.{OWNER}.release")),
        (3, format!("platform.v1.audit.decision.{OWNER}.deny")),
    ] {
        let stored = stream.get_raw_message(sequence).await.unwrap();
        assert_eq!(stored.subject.as_str(), subject);
        let record = AuditRecord::decode_from_slice(&stored.payload).unwrap();
        assert_eq!(record.request_id.as_deref(), Some(CORRELATION_ID));
    }
}
