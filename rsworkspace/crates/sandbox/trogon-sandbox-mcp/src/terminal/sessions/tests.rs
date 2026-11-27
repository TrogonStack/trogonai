use futures::StreamExt;
use futures::stream;
use trogon_sandbox_contract::{LeaseId, TerminalId};

use super::Sessions;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn sessions_are_keyed_by_lease_and_terminal() -> TestResult {
    let sessions = Sessions::default();
    let (l1, l2, t1) = (LeaseId::new("l1"), LeaseId::new("l2"), TerminalId::new("t1")?);
    sessions.attach(&l1, &t1, stream::empty().boxed());
    assert!(sessions.get(&l1, &t1).is_some());
    assert!(sessions.get(&l2, &t1).is_none());
    sessions.detach(&l1, &t1);
    assert!(sessions.get(&l1, &t1).is_none());
    Ok(())
}
