//! A lower lease epoch under `warn` is logged and admitted. This lives in its own test binary so the log
//! subscriber is global and no other test races it for tracing's per-callsite interest cache.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use serde_json::{Value, json};
use trogon_sandbox_protocol::{DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, OP_LEASE_DISPATCH};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn dispatch(signer: &DispatchSigner, epoch: u64) -> Result<Value, Box<dyn std::error::Error>> {
    let payload = serde_json::to_vec(&json!({
        "lease_id": "l1",
        "request": {"tenant": "t1"},
        "target": {"worker_id": "w1", "attempt_id": "a1", "epoch": epoch},
    }))?;
    Ok(serde_json::from_slice(
        &signer.sign_payload(&payload, OP_LEASE_DISPATCH)?,
    )?)
}

#[test]
fn warn_logs_a_lower_epoch_and_still_admits_it() -> TestResult {
    let captured = Captured::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish(),
    )?;
    let signer = DispatchSigner::new("k1", [7; 32]);
    let verifier = DispatchVerifier::new(EnforcementMode::Require)
        .with_key("k1", &signer.public_key_b64())?
        .with_epoch_mode(EpochMode::Warn);
    for epoch in [5, 2] {
        let mut value = dispatch(&signer, epoch)?;
        verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, SystemTime::now())?;
    }
    let log = String::from_utf8(captured.0.lock().unwrap_or_else(PoisonError::into_inner).clone())?;
    assert!(log.contains("older lease epoch"), "{log}");
    assert!(log.contains("held_epoch=5") && log.contains("seen_epoch=2"), "{log}");
    assert!(log.contains("enforced=false"), "{log}");
    Ok(())
}
