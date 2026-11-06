use super::*;
use crate::audit_readiness::AuditReadiness;

#[tokio::test]
async fn readiness_requires_a_fresh_integrity_ack_and_refuses_a_previously_healthy_failed_sink() {
    let (audit, sink) = audit();
    assert_eq!(audit.readiness().await, AuditReadiness::Ready);
    assert_eq!(sink.stored().len(), 2, "genesis and one readiness probe");
    let records = sink.stored_records();
    let genesis = sink.stored().first().expect("genesis").clone();
    let verified = verify_chain_range(
        ChainAnchor::Genesis { writer: writer(), chain: genesis.chain() },
        records,
    ).expect("the readiness probe preserves the hash chain");
    assert_eq!(verified.records(), 2);
    let probe = sink.stored().last().expect("probe").clone();
    assert_eq!(probe.subject(), "platform.v1.audit.integrity.egress_proxy.readiness_probe");
    sink.fail_all(SinkError::Unreachable);
    assert_eq!(audit.readiness().await, AuditReadiness::Unavailable);
    assert_eq!(audit.readiness().await, AuditReadiness::Degraded);
    sink.recover();
    audit.writer.drain_once().await;
    let before = sink.stored().len();
    assert_eq!(audit.readiness().await, AuditReadiness::Ready);
    assert_eq!(sink.stored().len(), before + 1, "recovery must acknowledge another fresh probe");
}
