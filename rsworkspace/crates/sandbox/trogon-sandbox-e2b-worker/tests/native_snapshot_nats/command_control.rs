use super::*;

#[tokio::test]
async fn restored_kill_commands_carry_host_pid_and_start_binding_and_refuse_unknown_ids() -> TestResult {
    let rig = Rig::new().await?;
    let execution_id = started(&rig.launch().await?)?;
    let receipt = native(&rig.capture().await?)?;
    rig.restore(TENANT, "lease-kill-fork", receipt.reference).await?;
    let request = KillCommandRequest {
        lease_id: LeaseId::new("lease-kill-fork"),
        execution_id,
        signal: UnixSignal::Kill,
    };
    let response: KillCommandResponse = rig
        .call("kill_command", "lease-kill-fork", serde_json::to_value(&request)?)
        .await?;
    assert_eq!(response, KillCommandResponse::Signaled);
    assert_eq!(*rig.control.kill_bindings.lock(), [(123, 456, true)]);
    let unknown = KillCommandRequest {
        execution_id: trogon_sandbox_contract::ExecutionId::new("unknown"),
        ..request
    };
    let response: KillCommandResponse = rig
        .call("kill_command", "lease-kill-fork", serde_json::to_value(unknown)?)
        .await?;
    assert_eq!(response, KillCommandResponse::NotFound);
    assert_eq!(rig.control.kill_bindings.lock().len(), 1);
    Ok(())
}
