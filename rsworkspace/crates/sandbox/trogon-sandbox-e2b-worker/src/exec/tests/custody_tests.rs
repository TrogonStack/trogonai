use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::SandboxUserName;

use super::{TestResult, record};
use crate::exec::command_custody::CommandCustody;
use crate::exec::command_phase::CommandPhase;
use crate::exec::command_record::CommandRecord;
use crate::exec::command_record_error::CommandRecordError;
use crate::exec::command_store::CommandStore;
use crate::exec::constants::COMMAND_CUSTODY_BUCKET;
use crate::exec::execution::Execution;
use crate::snapshots::{CapturedDeadline, CapturedExecution, ProcessIdentity};

fn bound(
    identity: ProcessIdentity,
    deadline: Option<CapturedDeadline>,
    same_id: bool,
) -> Result<CommandRecord, Box<dyn std::error::Error>> {
    let mut bound = record()?;
    let id = Execution::fresh_id();
    bound.phase = CommandPhase::Bound;
    bound.execution_id = Some(id.clone());
    bound.execution = Some(CapturedExecution {
        execution_id: if same_id {
            id.to_string()
        } else {
            Execution::fresh_id().to_string()
        },
        user: SandboxUserName::new("user")?,
        identity,
        max_output_bytes: 1024,
        deadline,
    });
    Ok(bound)
}

#[test]
fn a_bound_record_must_describe_the_process_it_names() -> TestResult {
    let real = ProcessIdentity { pid: 123, started: 456 };
    assert!(bound(real, Some(CapturedDeadline::Lease), true)?.validate().is_ok());
    for broken in [
        bound(real, Some(CapturedDeadline::Lease), false)?,
        bound(
            ProcessIdentity { pid: 1, started: 456 },
            Some(CapturedDeadline::Lease),
            true,
        )?,
        bound(
            ProcessIdentity { pid: 123, started: 0 },
            Some(CapturedDeadline::Lease),
            true,
        )?,
        bound(real, None, true)?,
    ] {
        assert!(matches!(broken.validate(), Err(CommandRecordError::NativeBinding)));
    }
    Ok(())
}

#[tokio::test]
async fn custody_reports_storage_failures_that_are_not_conflicts() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let custody = CommandCustody::default();
    custody.configure(context.clone())?;
    let reserved = record()?;
    assert!(custody.load(&reserved.owner, &reserved.launch_key).await?.is_none());
    let store = context.get_key_value(COMMAND_CUSTODY_BUCKET).await?;
    let key = "gone".to_owned();
    let stored = CommandCustody::create_in(&store, key.clone(), reserved.clone())
        .await?
        .ok_or("the record must be created")?;

    context.delete_stream(format!("KV_{COMMAND_CUSTODY_BUCKET}")).await?;
    assert!(
        CommandCustody::create_in(&store, "other".to_owned(), reserved.clone())
            .await
            .is_err()
    );
    assert!(
        CommandCustody::replace_in(&store, key, stored.revision, reserved)
            .await
            .is_err()
    );
    Ok(())
}
