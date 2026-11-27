use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use trogon_sandbox_contract::{FsOperation, FsViolation, LeaseId, LiteralPath};
use trogon_sandbox_egress_proxy::{ViolationPublishError, ViolationRateLimit, ViolationSink};

use super::{FsDenial, FsViolationReporter};

#[derive(Default)]
struct Collecting(Mutex<Vec<(String, Vec<u8>)>>);

impl Collecting {
    fn events(&self) -> Vec<(String, Vec<u8>)> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }
}

#[async_trait]
impl ViolationSink for Collecting {
    async fn publish(&self, subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((subject, payload));
        Ok(())
    }
}

fn denial(path: &str) -> Result<FsDenial, Box<dyn Error>> {
    Ok(FsDenial {
        path: LiteralPath::new(path)?,
        operation: FsOperation::Write,
    })
}

#[tokio::test]
async fn a_denial_is_published_on_the_lease_subject() -> Result<(), Box<dyn Error>> {
    let sink = Arc::new(Collecting::default());
    let reporter = FsViolationReporter::new(sink.clone(), ViolationRateLimit::default());
    reporter
        .report(&LeaseId::new("L1"), denial("/etc/hosts")?, SystemTime::UNIX_EPOCH)
        .await;
    let events = sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "sandbox.fs.violation.L1");
    let parsed: FsViolation = serde_json::from_slice(&events[0].1)?;
    assert_eq!(parsed.lease_id, LeaseId::new("L1"));
    assert_eq!(parsed.path.as_str(), "/etc/hosts");
    assert_eq!(parsed.operation, FsOperation::Write);
    Ok(())
}

#[tokio::test]
async fn denials_past_the_window_budget_are_dropped_per_lease() -> Result<(), Box<dyn Error>> {
    let sink = Arc::new(Collecting::default());
    let limit = ViolationRateLimit {
        max_events_per_window: 2,
        ..ViolationRateLimit::default()
    };
    let reporter = FsViolationReporter::new(sink.clone(), limit);
    let noisy = LeaseId::new("noisy");
    for _ in 0..5 {
        reporter.report(&noisy, denial("/x")?, SystemTime::UNIX_EPOCH).await;
    }
    reporter
        .report(&LeaseId::new("quiet"), denial("/x")?, SystemTime::UNIX_EPOCH)
        .await;
    assert_eq!(sink.events().len(), 3);
    Ok(())
}
