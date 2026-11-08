use super::*;

#[test]
fn decision_exposes_its_name_and_fault() {
    assert_eq!(Decision::Permitted.as_str(), "permitted");
    assert_eq!(Decision::Refused(Fault::NoPermit).as_str(), "refused");
    assert_eq!(Decision::Permitted.fault(), None);
    assert_eq!(Decision::Refused(Fault::SniMismatch).fault(), Some(Fault::SniMismatch));
}

struct Silent;

#[async_trait]
impl AuditSink for Silent {
    async fn record(&self, _event: &AuditEvent<'_>) -> Result<(), AuditError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_sink_without_background_work_returns_from_run_at_once() {
    let shutdown = CancellationToken::new();
    Silent.run(shutdown).await;
}
