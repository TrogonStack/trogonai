use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use trogon_sandbox_contract::{
    ArtifactType, CheckpointRunningRequest, ProviderDispatchPayload, ProviderLease, ReadFileResponse,
    RunCommandResponse, SandboxFreezeResponse, SnapshotExportError, SnapshotExportOutcome, SnapshotPushTarget,
    WriteFilesResponse,
};
use trogon_sandbox_protocol::subjects::{provider_checkpoint_running, provider_token};

use crate::check_failure::ensure;
use crate::held::{FreezeVerb, Held};
use crate::{CheckFailureError, CheckFuture, Target, ids, outcomes};

/// The native option block the unknown-key check sends under the provider's token.
#[derive(Serialize)]
struct UnknownNativeOption {
    conformance_unknown_option: bool,
}

pub(crate) fn lease_is_granted(target: &Target) -> CheckFuture<'_> {
    Box::pin(async move {
        Held::acquire(target).await?.release().await;
        Ok(())
    })
}

/// A paused lease keeps its files, refuses commands, and both verbs are idempotent.
pub(crate) fn pause_keeps_state_and_resume_restores_it<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let path = "/tmp/conformance-pause.txt";
        let written = l.write(path, b"kept").await?;
        let ok = matches!(written, WriteFilesResponse::Written);
        ensure(ok, "the file to be written", &written)?;
        for verb in [
            FreezeVerb::Pause,
            FreezeVerb::Pause,
            FreezeVerb::Resume,
            FreezeVerb::Resume,
        ] {
            let frozen = l.freeze(verb).await?;
            let done = matches!(frozen, SandboxFreezeResponse::Done);
            ensure(done, "the verb to be done", &frozen)?;
            let refused = l.run(&["echo", "x"], false).await?;
            let correct = refuses_when(&refused, verb);
            ensure(correct, "a paused lease to refuse a command", &refused)?;
        }
        let read = l.read(path, l.target.timeout).await?;
        let ok = matches!(&read, ReadFileResponse::Content { data } if data == b"kept");
        ensure(ok, "the state to survive the pause", &read)?;
        let out = outcomes::completed(l.run(&["echo", "back"], false).await?)?;
        ensure(out.exit_code == 0, "a resumed lease to run a command", &out)
    })
}

/// A worker that cannot snapshot a running lease must say so with `SnapshotExportError::Unsupported` so the broker
/// can map it to a typed capability answer.
/// A command is refused while paused and runs once resumed.
fn refuses_when(reply: &RunCommandResponse, verb: FreezeVerb) -> bool {
    matches!(reply, RunCommandResponse::Denied(_)) == matches!(verb, FreezeVerb::Pause)
}

pub(crate) fn unsupported_checkpoint_running_gets_a_typed_reply<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let request = CheckpointRunningRequest {
            lease_id: l.id.clone(),
            target: SnapshotPushTarget {
                registry: "registry.invalid".into(),
                repository: "conformance/snapshots".into(),
                artifact_type: ArtifactType::SandboxRootfsV1,
            },
            native_name: None,
        };
        let outcome: SnapshotExportOutcome = l
            .target
            .call(
                provider_checkpoint_running(l.target.kind, &l.id),
                &request,
                l.target.timeout,
            )
            .await?;
        let ok = matches!(
            outcome,
            SnapshotExportOutcome::Err(SnapshotExportError::Unsupported { .. })
        );
        ensure(ok, "a typed unsupported reply", &outcome)
    })
}

pub(crate) fn unknown_native_key_is_refused_with_a_typed_error(target: &Target) -> CheckFuture<'_> {
    Box::pin(async move {
        let mut request = target.lease_request()?;
        let native = BTreeMap::from([(
            provider_token(target.kind),
            UnknownNativeOption {
                conformance_unknown_option: true,
            },
        )]);
        request.native = Some(serde_json::to_value(native).map_err(CheckFailureError::Encode)?);
        let id = ids::fresh_lease_id();
        let assignment = target.assignment(&id)?;
        let payload = ProviderDispatchPayload {
            lease_id: id.clone(),
            request,
            correlation_id: None,
            resolved_drives: vec![],
            target: assignment.clone(),
        };
        let reply: Value = target
            .call(target.dispatch_subject(assignment.as_ref()), &payload, target.timeout)
            .await?;
        let refusal = reply.get("native_error").and_then(Value::as_str);
        let names_key = refusal.is_some_and(|reason| reason.contains("conformance_unknown_option"));
        let result = ensure(names_key, "a refusal that names the unknown key", &reply);
        if let Ok(native) = serde_json::from_value::<ProviderLease>(reply) {
            let admitted = target.admit_grant(&id, &native);
            if admitted.is_ok() || assignment.is_some() {
                target.cancel(&id).await;
            }
        }
        result
    })
}

pub(crate) fn cancel_releases_the_lease(target: &Target) -> CheckFuture<'_> {
    Box::pin(async move {
        let held = Held::acquire(target).await?;
        held.plant_state("cancel").await?;
        target.cancel(&held.id).await;
        // A released lease may go silent (request times out) or be denied; it must never keep serving the state it
        // held.
        let deadline = Instant::now() + target.timeout;
        loop {
            let state = held.read_state("cancel", Duration::from_secs(1)).await;
            if !matches!(state, Ok(Some(_))) {
                return Ok(());
            }
            let in_time = Instant::now() <= deadline;
            ensure(in_time, "the lease to stop serving its state", &state)?;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
}

pub(crate) fn redispatching_a_lease_keeps_its_state<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        l.plant_state("redispatch").await?;
        let first = l.redispatch().await?;
        let second = l.redispatch().await?;
        let same = first.sandbox_id == second.sandbox_id;
        let ids = (first.sandbox_id, second.sandbox_id);
        ensure(same, "a re-dispatch to keep one sandbox", &ids)?;
        let state = l.read_state("redispatch", l.target.timeout).await?;
        let kept = state.as_deref() == Some(b"kept".as_slice());
        ensure(kept, "a re-dispatch to keep the lease's state", &state)
    })
}
