use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CommandDeadline, CommandExecution, ExecutionId,
    LaunchCancellation, LaunchKey, LeaseError, LeaseId, ProviderKind, RunCommandRequest, SandboxUserName, StdinMode,
};
use trogon_sandbox_protocol::subjects;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::scripted_store_tests::{Fault, Op, ScriptedStore};
use super::{Script, TestResult, exited, fixture, mount_file, mount_script, owner, pid_and_exit, run_request};
use crate::LeaseSandbox;
use crate::exec::command_owner::CommandOwner;
use crate::exec::command_phase::CommandPhase;
use crate::exec::command_record::CommandRecord;
use crate::exec::constants::{KILL_SCRIPT, LAUNCH_SCRIPT, MAX_CUSTODY_ATTEMPTS, PROCESS_IDENTITY, RELEASE_SCRIPT};
use crate::exec::e2b_exec::E2bExec;
use crate::exec::launch_fingerprint::LaunchFingerprint;
use crate::snapshots::{CapturedDeadline, CapturedExecution, ProcessIdentity};

pub(super) struct Controlled {
    pub(super) server: MockServer,
    pub(super) exec: E2bExec,
    pub(super) sandbox: LeaseSandbox,
    pub(super) store: ScriptedStore,
    pub(super) owner: CommandOwner,
}

pub(super) async fn controlled() -> Result<Controlled, Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    let (mut exec, sandbox) = fixture(&server)?;
    let store = ScriptedStore::default();
    exec.command_custody = Box::new(store.clone());
    Ok(Controlled {
        server,
        exec,
        sandbox,
        store,
        owner: owner("lease-1", "sbx1")?,
    })
}

impl Controlled {
    pub(super) async fn guest(&self) -> TestResult {
        mount_script(&self.server, LAUNCH_SCRIPT, pid_and_exit()?).await;
        mount_script(&self.server, RELEASE_SCRIPT, exited(0)?).await;
        mount_script(&self.server, PROCESS_IDENTITY, pid_and_exit()?).await;
        Ok(())
    }

    pub(super) fn cancel_request(&self, key: &str) -> Result<CancelCommandLaunchRequest, Box<dyn std::error::Error>> {
        Ok(CancelCommandLaunchRequest {
            lease_id: LeaseId::new("lease-1"),
            launch_key: LaunchKey::new(key)?,
        })
    }

    pub(super) fn seed(
        &self,
        key: &str,
        phase: CommandPhase,
        cancel_requested: bool,
        execution: bool,
    ) -> Result<CommandRecord, Box<dyn std::error::Error>> {
        let mut record = CommandRecord::reserved(
            self.owner.clone(),
            LaunchKey::new(key)?,
            LaunchFingerprint::of(&controlled_request(key)?)?,
        );
        record.phase = phase;
        record.cancel_requested = cancel_requested;
        if execution {
            record.execution = Some(captured(&record)?);
        }
        self.store.seed(record.clone());
        Ok(record)
    }
}

pub(super) fn controlled_request(key: &str) -> Result<RunCommandRequest, Box<dyn std::error::Error>> {
    Ok(RunCommandRequest {
        launch_key: Some(LaunchKey::new(key)?),
        require_launch_cancellation: LaunchCancellation::Required,
        execution: CommandExecution::Detached {
            stdin: StdinMode::Closed,
            deadline: CommandDeadline::Lease,
            process: None,
        },
        ..run_request(&["sleep", "60"])
    })
}

fn captured(record: &CommandRecord) -> Result<CapturedExecution, Box<dyn std::error::Error>> {
    Ok(CapturedExecution {
        execution_id: record
            .execution_id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        user: SandboxUserName::new("user")?,
        identity: ProcessIdentity { pid: 123, started: 456 },
        max_output_bytes: 1024,
        deadline: Some(CapturedDeadline::Lease),
    })
}

fn is_backend(result: &Result<ExecutionId, LeaseError>) -> bool {
    matches!(result, Err(LeaseError::Backend { .. }))
}

fn is_invalid(result: &Result<ExecutionId, LeaseError>) -> bool {
    matches!(result, Err(LeaseError::InvalidSpec { .. }))
}

fn pending(response: &CancelCommandLaunchResponse, fragment: &str) -> bool {
    matches!(response, CancelCommandLaunchResponse::Pending { message } if message.contains(fragment))
}

fn canceled_with_execution(response: &CancelCommandLaunchResponse) -> bool {
    matches!(
        response,
        CancelCommandLaunchResponse::Canceled { execution_id: Some(_) }
    )
}

#[tokio::test]
async fn a_keyed_launch_starts_replays_and_is_restored_by_a_new_worker() -> TestResult {
    let c = controlled().await?;
    c.guest().await?;
    let request = controlled_request("key-1")?;
    let id = c.exec.start_controlled(&c.sandbox, &request, c.owner.clone()).await?;
    let stored = c
        .store
        .record(&c.owner, &LaunchKey::new("key-1")?)
        .ok_or("the record must exist")?;
    assert_eq!(stored.phase, CommandPhase::Running);
    let replay = c.exec.start_controlled(&c.sandbox, &request, c.owner.clone()).await?;
    assert_eq!(replay, id);

    let (mut successor, sandbox) = fixture(&c.server)?;
    successor.command_custody = Box::new(c.store.clone());
    let restored = successor.start_controlled(&sandbox, &request, c.owner.clone()).await?;
    assert_eq!(restored, id);
    assert!(successor.execution(&c.owner.lease_id, &id).await.is_some());
    Ok(())
}

#[tokio::test]
async fn a_keyed_launch_refuses_records_it_cannot_continue() -> TestResult {
    let c = controlled().await?;
    c.guest().await?;
    let canceled = CommandRecord::canceled_before_start(c.owner.clone(), LaunchKey::new("canceled")?);
    c.store.seed(canceled);
    let result = c
        .exec
        .start_controlled(&c.sandbox, &controlled_request("canceled")?, c.owner.clone())
        .await;
    assert!(is_invalid(&result));

    c.seed("requested", CommandPhase::Reserved, true, false)?;
    let result = c
        .exec
        .start_controlled(&c.sandbox, &controlled_request("requested")?, c.owner.clone())
        .await;
    assert!(is_invalid(&result));

    c.seed("foreign", CommandPhase::Reserved, false, false)?;
    let other = RunCommandRequest {
        command: vec!["other".to_owned()],
        ..controlled_request("foreign")?
    };
    let result = c.exec.start_controlled(&c.sandbox, &other, c.owner.clone()).await;
    assert!(is_invalid(&result));

    c.seed("bound", CommandPhase::Bound, false, true)?;
    let result = c
        .exec
        .start_controlled(&c.sandbox, &controlled_request("bound")?, c.owner.clone())
        .await;
    assert!(is_backend(&result));

    c.seed("hollow", CommandPhase::Running, false, false)?;
    let result = c
        .exec
        .start_controlled(&c.sandbox, &controlled_request("hollow")?, c.owner.clone())
        .await;
    assert!(is_backend(&result));
    Ok(())
}

#[tokio::test]
async fn a_keyed_launch_retries_contended_custody_and_gives_up() -> TestResult {
    let c = controlled().await?;
    c.guest().await?;
    c.store.script(Op::Create, [Fault::Conflict]);
    c.store.script(Op::Replace, [Fault::Conflict]);
    let started = c
        .exec
        .start_controlled(&c.sandbox, &controlled_request("retry")?, c.owner.clone())
        .await;
    assert!(started.is_ok());

    c.seed("contended", CommandPhase::Reserved, false, false)?;
    c.store
        .script(Op::Replace, (0..MAX_CUSTODY_ATTEMPTS).map(|_| Fault::Conflict));
    let result = c
        .exec
        .start_controlled(&c.sandbox, &controlled_request("contended")?, c.owner.clone())
        .await;
    assert!(is_backend(&result));
    Ok(())
}

#[tokio::test]
async fn a_launch_that_loses_custody_before_release_is_refused() -> TestResult {
    let c = controlled().await?;
    c.guest().await?;
    c.store.script(
        Op::Load,
        [
            Fault::Pass,
            Fault::Rewrite(|record| record.phase = CommandPhase::Running),
        ],
    );
    let result = c
        .exec
        .start_controlled(&c.sandbox, &controlled_request("lost-bind")?, c.owner.clone())
        .await;
    assert!(is_backend(&result));

    c.store.script(Op::Replace, [Fault::Pass, Fault::Conflict]);
    let result = c
        .exec
        .start_controlled(&c.sandbox, &controlled_request("retried-bind")?, c.owner.clone())
        .await;
    assert!(result.is_ok());

    c.store.script(
        Op::Replace,
        std::iter::once(Fault::Pass).chain((0..MAX_CUSTODY_ATTEMPTS).map(|_| Fault::Conflict)),
    );
    let result = c
        .exec
        .start_controlled(&c.sandbox, &controlled_request("exhausted-bind")?, c.owner.clone())
        .await;
    assert!(is_backend(&result));

    c.store.script(Op::Replace, [Fault::Pass, Fault::Pass, Fault::Conflict]);
    let result = c
        .exec
        .start_controlled(&c.sandbox, &controlled_request("lost-release")?, c.owner.clone())
        .await;
    assert!(is_backend(&result));
    Ok(())
}

#[tokio::test]
async fn cancellation_before_any_launch_settles_or_stays_pending() -> TestResult {
    let c = controlled().await?;
    let key = |name: &str| c.cancel_request(name);

    c.store.script(Op::Create, [Fault::Conflict]);
    let response = c.exec.cancel_controlled(&c.sandbox, &key("a")?, c.owner.clone()).await;
    assert_eq!(response, CancelCommandLaunchResponse::Canceled { execution_id: None });

    c.store.script(Op::Create, [Fault::Fail]);
    let response = c.exec.cancel_controlled(&c.sandbox, &key("b")?, c.owner.clone()).await;
    assert!(pending(&response, "intent"));

    c.store.script(Op::Load, [Fault::Fail]);
    let response = c.exec.cancel_controlled(&c.sandbox, &key("c")?, c.owner.clone()).await;
    assert!(pending(&response, "ownership"));

    c.seed("d", CommandPhase::Reserved, false, false)?;
    let response = c.exec.cancel_controlled(&c.sandbox, &key("d")?, c.owner.clone()).await;
    assert_eq!(response, CancelCommandLaunchResponse::Canceled { execution_id: None });

    c.seed("e", CommandPhase::Reserved, false, false)?;
    c.store.script(Op::Replace, [Fault::Conflict]);
    let response = c.exec.cancel_controlled(&c.sandbox, &key("e")?, c.owner.clone()).await;
    assert_eq!(response, CancelCommandLaunchResponse::Canceled { execution_id: None });

    c.seed("f", CommandPhase::Reserved, false, false)?;
    c.store.script(Op::Replace, [Fault::Fail]);
    let response = c.exec.cancel_controlled(&c.sandbox, &key("f")?, c.owner.clone()).await;
    assert!(pending(&response, "intent"));

    c.seed("g", CommandPhase::Reserved, false, false)?;
    c.store
        .script(Op::Replace, (0..MAX_CUSTODY_ATTEMPTS).map(|_| Fault::Conflict));
    let response = c.exec.cancel_controlled(&c.sandbox, &key("g")?, c.owner.clone()).await;
    assert!(pending(&response, "concurrently"));

    c.seed("h", CommandPhase::Preparing, false, false)?;
    let response = c.exec.cancel_controlled(&c.sandbox, &key("h")?, c.owner.clone()).await;
    assert!(pending(&response, "bootstrap"));
    Ok(())
}

async fn mount_absent(server: &MockServer) -> TestResult {
    mount_script(server, PROCESS_IDENTITY, exited(44)?).await;
    mount_file(server, "/exit", Some("0\n")).await;
    Ok(())
}

/// The process is seen running once, then gone with a recorded exit status.
async fn mount_running_once(server: &MockServer) -> TestResult {
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(Script(PROCESS_IDENTITY))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(pid_and_exit()?))
        .up_to_n_times(1)
        .mount(server)
        .await;
    mount_absent(server).await
}

#[tokio::test]
async fn cancellation_kills_the_exact_process_and_settles_its_record() -> TestResult {
    let c = controlled().await?;
    mount_running_once(&c.server).await?;
    mount_script(&c.server, KILL_SCRIPT, exited(0)?).await;
    c.seed("run", CommandPhase::Running, false, true)?;
    let response = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("run")?, c.owner.clone())
        .await;
    assert!(canceled_with_execution(&response));
    let again = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("run")?, c.owner.clone())
        .await;
    assert!(canceled_with_execution(&again));
    Ok(())
}

#[tokio::test]
async fn cancellation_stays_pending_while_native_custody_is_unconfirmed() -> TestResult {
    let c = controlled().await?;
    c.seed("unknown", CommandPhase::Running, true, true)?;
    let response = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("unknown")?, c.owner.clone())
        .await;
    assert!(pending(&response, "identity"));

    mount_script(&c.server, PROCESS_IDENTITY, pid_and_exit()?).await;
    mount_script(&c.server, KILL_SCRIPT, exited(1)?).await;
    let response = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("unknown")?, c.owner.clone())
        .await;
    assert!(pending(&response, "remains unconfirmed"));

    c.server.reset().await;
    mount_script(&c.server, PROCESS_IDENTITY, pid_and_exit()?).await;
    mount_script(&c.server, KILL_SCRIPT, exited(0)?).await;
    let response = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("unknown")?, c.owner.clone())
        .await;
    assert!(pending(&response, "completion could not be confirmed"));
    Ok(())
}

#[tokio::test]
async fn cancellation_survives_a_contended_final_settlement() -> TestResult {
    let c = controlled().await?;
    mount_absent(&c.server).await?;
    c.seed("settle", CommandPhase::Running, true, true)?;
    c.store.script(Op::Replace, [Fault::Fail]);
    let response = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("settle")?, c.owner.clone())
        .await;
    assert!(pending(&response, "durably settled"));

    c.store.script(Op::Replace, [Fault::Conflict]);
    let response = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("settle")?, c.owner.clone())
        .await;
    assert!(canceled_with_execution(&response));

    c.seed("vanish", CommandPhase::Running, true, true)?;
    c.store.script(Op::Replace, [Fault::Conflict]);
    c.store.script(Op::Load, [Fault::Pass, Fault::Fail]);
    let response = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("vanish")?, c.owner.clone())
        .await;
    assert!(pending(&response, "became unavailable"));

    c.seed("settled-elsewhere", CommandPhase::Running, true, true)?;
    c.store.script(Op::Replace, [Fault::Conflict]);
    c.store.script(
        Op::Load,
        [
            Fault::Pass,
            Fault::Rewrite(|record| record.phase = CommandPhase::Canceled),
        ],
    );
    let response = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("settled-elsewhere")?, c.owner.clone())
        .await;
    assert!(canceled_with_execution(&response));

    c.seed("unbound", CommandPhase::Running, true, true)?;
    c.store.script(Op::Replace, [Fault::Conflict]);
    c.store.script(
        Op::Load,
        [Fault::Pass, Fault::Rewrite(|record| record.execution = None)],
    );
    let response = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("unbound")?, c.owner.clone())
        .await;
    assert!(pending(&response, "bootstrap"));

    c.seed("churn", CommandPhase::Running, true, true)?;
    c.store
        .script(Op::Replace, (0..MAX_CUSTODY_ATTEMPTS).map(|_| Fault::Conflict));
    let response = c
        .exec
        .cancel_controlled(&c.sandbox, &c.cancel_request("churn")?, c.owner.clone())
        .await;
    assert!(pending(&response, "concurrently"));
    Ok(())
}

#[tokio::test]
async fn a_cancellation_scoped_to_another_request_is_denied() -> TestResult {
    let c = controlled().await?;
    let request = c.cancel_request("scoped")?;
    let subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &request.lease_id);
    let (scope, payload) = super::command_scope_tests::signed(&request, &request.lease_id, &subject)?;
    let other = c.cancel_request("elsewhere")?;
    let response = c
        .exec
        .handle_cancel_command_launch_with_scope(&c.sandbox, &other, &scope, &payload)
        .await;
    assert!(matches!(response, CancelCommandLaunchResponse::Denied(_)));
    let response = c
        .exec
        .handle_cancel_command_launch_with_scope(&c.sandbox, &request, &scope, &payload)
        .await;
    assert_eq!(response, CancelCommandLaunchResponse::Canceled { execution_id: None });
    Ok(())
}
