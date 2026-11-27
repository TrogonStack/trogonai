mod support;

use futures::future::try_join_all;
use serde_json::{Value, json};
use support::*;
use trogon_sandbox_contract::{
    CreateSandboxGroupResponse, CreateSandboxUserResponse, FileOpResponse, GetSandboxDomainResponse,
    KillCommandResponse, LeaseError, ModifyGroupMembershipResponse, NetworkPolicyUpdateResponse, PollCommandResponse,
    ReadFileResponse, RestartCommandResponse, RunCommandResponse, StopCommandResponse, TerminalResponse,
    WriteFilesResponse, WriteStdinResponse,
};
use trogon_sandbox_k8s_worker::K8sVerb;

fn typed<T: serde::de::DeserializeOwned>(value: &Value) -> Boxed<T> {
    Ok(serde_json::from_value(value.clone())?)
}

fn payload_for(verb: K8sVerb) -> Value {
    match verb {
        K8sVerb::NetworkPolicyUpdate => json!({"network_policy": "deny_all"}),
        K8sVerb::CheckpointRunning => {
            json!({"target": {"registry": "r", "repository": "p", "artifact_type": "sandbox_rootfs_v1"}})
        }
        K8sVerb::Rollback => {
            json!({"snapshot": {"registry": "r", "repository": "p", "digest": "sha256:00", "artifact_type": "sandbox_rootfs_v1"}})
        }
        K8sVerb::RunCommand => json!({"command": ["true"]}),
        K8sVerb::PollCommand | K8sVerb::KillCommand | K8sVerb::StopCommand | K8sVerb::RestartCommand => {
            json!({"execution_id": "e-1"})
        }
        K8sVerb::WriteStdin => json!({"execution_id": "e-1", "seq": 0, "data": [1]}),
        K8sVerb::WriteFiles => json!({"files": []}),
        K8sVerb::ReadFile => json!({"path": "/a"}),
        K8sVerb::FileOp => json!({"op": {"op": "stat", "path": "/a"}}),
        K8sVerb::CreateUser => json!({"username": "alice"}),
        K8sVerb::CreateGroup => json!({"groupname": "devs"}),
        K8sVerb::ModifyGroupMembership => json!({"username": "alice", "groupname": "devs", "action": "add"}),
        K8sVerb::GetDomain => json!({"port": 80}),
        K8sVerb::SshAttach => json!({"session_id": "s-1", "public_key": "ssh-ed25519 AAAA", "ttl_seconds": 60}),
        K8sVerb::SshDetach => json!({"session_id": "s-1"}),
        K8sVerb::Terminal => json!({"terminal_id": "t-1", "op": "close"}),
    }
}

#[tokio::test]
async fn a_lease_nobody_holds_gets_no_answer_to_any_verb() -> TestResult {
    let mut running = Running::start().await?;
    let silences = K8sVerb::ALL.map(|verb| {
        let mut payload = payload_for(verb);
        payload["lease_id"] = json!("lease-absent");
        running.silent(
            verb_subject(verb, "lease-absent"),
            serde_json::to_vec(&payload).unwrap_or_default(),
        )
    });
    try_join_all(silences).await?;
    running.stop().await
}

#[tokio::test]
async fn a_request_that_cannot_be_answered_is_dropped() -> TestResult {
    let mut running = Running::start().await?;
    running.grant("lease-1").await?;
    // No reply subject, then a reply subject with a payload that does not decode; all verbs wait out the
    // silence window together.
    for verb in K8sVerb::ALL {
        running
            .publish(verb_subject(verb, "lease-1"), &json!({"lease_id": "lease-1"}))
            .await?;
    }
    let silences = K8sVerb::ALL.map(|verb| running.silent(verb_subject(verb, "lease-1"), b"not json".to_vec()));
    try_join_all(silences).await?;
    // The worker is still serving afterwards.
    let reply = running.verb(K8sVerb::GetDomain, "lease-1", json!({"port": 80})).await?;
    assert_eq!(
        typed::<GetSandboxDomainResponse>(&reply)?,
        GetSandboxDomainResponse::Unsupported
    );
    running.stop().await
}

#[tokio::test]
async fn commands_run_poll_and_end() -> TestResult {
    let mut running = Running::start().await?;
    running.grant("lease-1").await?;
    running
        .world
        .cluster
        .script(ExecScript::new("run-ok").stdout(b"hello").exit(0));

    let completed = running
        .verb(
            K8sVerb::RunCommand,
            "lease-1",
            json!({"command": ["run-ok"], "timeout_seconds": 10}),
        )
        .await?;
    let RunCommandResponse::Completed(output) = typed(&completed)? else {
        return Err("not completed".into());
    };
    assert_eq!(output.stdout, b"hello");

    let started = running
        .verb(
            K8sVerb::RunCommand,
            "lease-1",
            json!({"command": ["run-ok"], "detached": true, "timeout_seconds": 10}),
        )
        .await?;
    let RunCommandResponse::Started(started) = typed(&started)? else {
        return Err("not started".into());
    };
    let execution = started.execution_id.as_str().to_owned();
    let polled = loop {
        let reply = running
            .verb(K8sVerb::PollCommand, "lease-1", json!({"execution_id": execution}))
            .await?;
        let polled: PollCommandResponse = typed(&reply)?;
        if !matches!(polled, PollCommandResponse::Running) {
            break polled;
        }
    };
    assert!(matches!(polled, PollCommandResponse::Completed(_)), "{polled:?}");
    let unknown = running
        .verb(K8sVerb::PollCommand, "lease-1", json!({"execution_id": "unknown"}))
        .await?;
    assert!(matches!(typed(&unknown)?, PollCommandResponse::NotFound));

    let killed = running
        .verb(K8sVerb::KillCommand, "lease-1", json!({"execution_id": "unknown"}))
        .await?;
    assert!(matches!(typed(&killed)?, KillCommandResponse::NotFound));
    let stopped = running
        .verb(
            K8sVerb::StopCommand,
            "lease-1",
            json!({"execution_id": "unknown", "grace_seconds": 100000}),
        )
        .await?;
    assert!(matches!(typed(&stopped)?, StopCommandResponse::NotFound));
    let restarted = running
        .verb(K8sVerb::RestartCommand, "lease-1", json!({"execution_id": "unknown"}))
        .await?;
    assert!(matches!(typed(&restarted)?, RestartCommandResponse::NotFound));
    let written = running
        .verb(
            K8sVerb::WriteStdin,
            "lease-1",
            json!({"execution_id": "unknown", "seq": 0, "data": [1, 2]}),
        )
        .await?;
    assert!(matches!(typed(&written)?, WriteStdinResponse::NotFound));
    running.stop().await
}

#[tokio::test]
async fn unsupported_command_options_and_failures_are_denied() -> TestResult {
    let mut running = Running::start().await?;
    running.grant("lease-1").await?;
    let cancellable = running
        .verb(
            K8sVerb::RunCommand,
            "lease-1",
            json!({"command": ["x"], "require_launch_cancellation": true}),
        )
        .await?;
    assert!(matches!(
        typed(&cancellable)?,
        RunCommandResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    let leased = running
        .verb(
            K8sVerb::RunCommand,
            "lease-1",
            json!({"command": ["x"], "detached": true, "deadline": "lease"}),
        )
        .await?;
    assert!(matches!(
        typed(&leased)?,
        RunCommandResponse::Denied(LeaseError::InvalidSpec { .. })
    ));

    // Output beyond the output cap is trimmed to fit one reply, and says so.
    running
        .world
        .cluster
        .script(ExecScript::new("run-big").stdout(&vec![b'x'; 2 * 1024 * 1024]));
    let big = running
        .verb(
            K8sVerb::RunCommand,
            "lease-1",
            json!({"command": ["run-big"], "timeout_seconds": 10}),
        )
        .await?;
    let RunCommandResponse::Completed(output) = typed(&big)? else {
        return Err("an oversized output must be trimmed, not refused".into());
    };
    assert!(output.truncated.is_truncated());
    assert!(output.stdout.len() < 2 * 1024 * 1024);

    running.world.cluster.state().scripts.clear();
    running.world.cluster.script(ExecScript::new("").refuse(503));
    let refused = running
        .verb(
            K8sVerb::RunCommand,
            "lease-1",
            json!({"command": ["run-refused"], "timeout_seconds": 10}),
        )
        .await?;
    assert!(matches!(
        typed(&refused)?,
        RunCommandResponse::Denied(LeaseError::Backend { .. })
    ));
    running.stop().await
}

#[tokio::test]
async fn files_are_written_read_and_listed() -> TestResult {
    let mut running = Running::start().await?;
    running.grant("lease-1").await?;

    let written = running
        .verb(
            K8sVerb::WriteFiles,
            "lease-1",
            json!({"files": [{"path": "/work/it's", "content": [104, 105], "mode": 384}]}),
        )
        .await?;
    assert!(matches!(typed(&written)?, WriteFilesResponse::Written));
    assert!(
        running
            .world
            .cluster
            .state()
            .execs
            .iter()
            .any(|command| command.contains("chmod"))
    );
    let as_user = running
        .verb(
            K8sVerb::WriteFiles,
            "lease-1",
            json!({"files": [{"path": "/work/a", "content": [1]}], "as_user": "alice"}),
        )
        .await?;
    assert!(matches!(typed(&as_user)?, WriteFilesResponse::Written));

    running
        .world
        .cluster
        .script(ExecScript::new("mkdir -p").exit(1).stderr(b"read only"));
    let failed = running
        .verb(
            K8sVerb::WriteFiles,
            "lease-1",
            json!({"files": [{"path": "/ro/a", "content": [1]}]}),
        )
        .await?;
    assert!(matches!(typed(&failed)?, WriteFilesResponse::Denied(_)));
    running.world.cluster.state().scripts.clear();
    running.world.cluster.script(ExecScript::new("").refuse(503));
    let refused = running
        .verb(
            K8sVerb::WriteFiles,
            "lease-1",
            json!({"files": [{"path": "/a", "content": [1]}]}),
        )
        .await?;
    assert!(matches!(typed(&refused)?, WriteFilesResponse::Denied(_)));

    running.world.cluster.state().scripts.clear();
    running
        .world
        .cluster
        .script(ExecScript::new("cat /work/data").stdout(b"contents"));
    let read = running
        .verb(K8sVerb::ReadFile, "lease-1", json!({"path": "/work/data"}))
        .await?;
    assert!(matches!(typed(&read)?, ReadFileResponse::Content { data } if data == b"contents"));
    running
        .world
        .cluster
        .script(ExecScript::new("runuser").stdout(b"as-user"));
    let as_user = running
        .verb(
            K8sVerb::ReadFile,
            "lease-1",
            json!({"path": "/home/a", "as_user": "alice"}),
        )
        .await?;
    assert!(matches!(typed(&as_user)?, ReadFileResponse::Content { data } if data == b"as-user"));
    running
        .world
        .cluster
        .script(ExecScript::new("cat /missing").exit(1).stderr(b"no such file"));
    let missing = running
        .verb(K8sVerb::ReadFile, "lease-1", json!({"path": "/missing"}))
        .await?;
    assert!(matches!(typed(&missing)?, ReadFileResponse::Denied(_)));
    running.world.cluster.state().scripts.clear();
    running.world.cluster.script(ExecScript::new("").refuse(503));
    let refused = running
        .verb(K8sVerb::ReadFile, "lease-1", json!({"path": "/x"}))
        .await?;
    assert!(matches!(typed(&refused)?, ReadFileResponse::Denied(_)));

    let listed = running
        .verb(K8sVerb::FileOp, "lease-1", json!({"op": {"op": "stat", "path": "/a"}}))
        .await?;
    assert!(
        matches!(typed(&listed)?, FileOpResponse::Unsupported { .. }),
        "{listed}"
    );
    running.stop().await
}

#[tokio::test]
async fn accounts_are_created_for_a_held_lease() -> TestResult {
    let mut running = Running::start().await?;
    running.grant("lease-1").await?;

    running.world.cluster.script(ExecScript::new("id -u").exit(1));
    let user = running
        .verb(K8sVerb::CreateUser, "lease-1", json!({"username": "alice"}))
        .await?;
    assert!(matches!(typed(&user)?, CreateSandboxUserResponse::Created { .. }));
    running.world.cluster.script(ExecScript::new("getent group").exit(2));
    let group = running
        .verb(K8sVerb::CreateGroup, "lease-1", json!({"groupname": "devs"}))
        .await?;
    assert!(matches!(typed(&group)?, CreateSandboxGroupResponse::Created { .. }));
    let member = running
        .verb(
            K8sVerb::ModifyGroupMembership,
            "lease-1",
            json!({"username": "alice", "groupname": "devs", "action": "add"}),
        )
        .await?;
    assert!(
        matches!(typed(&member)?, ModifyGroupMembershipResponse::Applied),
        "{member}"
    );

    running.world.cluster.state().scripts.clear();
    running.world.cluster.script(ExecScript::new("").refuse(503));
    let user = running
        .verb(K8sVerb::CreateUser, "lease-1", json!({"username": "bob"}))
        .await?;
    assert!(matches!(typed(&user)?, CreateSandboxUserResponse::Denied(_)));
    let group = running
        .verb(K8sVerb::CreateGroup, "lease-1", json!({"groupname": "ops"}))
        .await?;
    assert!(matches!(typed(&group)?, CreateSandboxGroupResponse::Denied(_)));
    let member = running
        .verb(
            K8sVerb::ModifyGroupMembership,
            "lease-1",
            json!({"username": "bob", "groupname": "ops", "action": "remove"}),
        )
        .await?;
    assert!(matches!(typed(&member)?, ModifyGroupMembershipResponse::Denied(_)));
    running.stop().await
}

#[tokio::test]
async fn network_policies_are_applied_or_refused() -> TestResult {
    let mut running = Running::start().await?;
    running.grant("lease-1").await?;
    let applied = running
        .verb(
            K8sVerb::NetworkPolicyUpdate,
            "lease-1",
            json!({"network_policy": "deny_all"}),
        )
        .await?;
    assert!(matches!(typed(&applied)?, NetworkPolicyUpdateResponse::Applied));
    assert_eq!(running.world.cluster.names("networkpolicies").len(), 1);

    let hostnames = running
        .verb(
            K8sVerb::NetworkPolicyUpdate,
            "lease-1",
            json!({"network_policy": {"user_defined": {"rules": [{"domain": "example.com"}], "allow_ranges": [], "deny_ranges": []}}}),
        )
        .await?;
    assert!(
        matches!(
            typed(&hostnames)?,
            NetworkPolicyUpdateResponse::Denied(LeaseError::InvalidNetworkPolicy { .. })
        ),
        "{hostnames}"
    );

    running
        .world
        .cluster
        .state()
        .fail_creates
        .push("networkpolicies".into());
    let failed = running
        .verb(
            K8sVerb::NetworkPolicyUpdate,
            "lease-1",
            json!({"network_policy": "allow_all"}),
        )
        .await?;
    let failed: NetworkPolicyUpdateResponse = typed(&failed)?;
    assert!(matches!(
        failed,
        NetworkPolicyUpdateResponse::Applied | NetworkPolicyUpdateResponse::Denied(_)
    ));
    running.stop().await
}

#[tokio::test]
async fn ssh_sessions_attach_and_detach_through_the_bridge() -> TestResult {
    let mut running = Running::start().await?;
    running.grant("lease-1").await?;
    let attached = running
        .ask(
            verb_subject(K8sVerb::SshAttach, "lease-1"),
            &json!({"session_id": "s-1", "lease_id": "lease-1", "public_key": "ssh-ed25519 AAAA", "ttl_seconds": 60}),
        )
        .await?;
    assert_eq!(attached["ok"], true, "{attached}");
    assert!(
        running
            .world
            .cluster
            .state()
            .execs
            .iter()
            .any(|command| command.contains("authorized_keys"))
    );

    // A key that cannot be installed does not refuse the session.
    running.world.cluster.script(ExecScript::new("authorized_keys").exit(1));
    let attached = running
        .ask(
            verb_subject(K8sVerb::SshAttach, "lease-1"),
            &json!({"session_id": "s-2", "lease_id": "lease-1", "public_key": "ssh-ed25519 BBBB", "ttl_seconds": 60}),
        )
        .await?;
    assert_eq!(attached["ok"], true, "{attached}");

    let detached = running
        .ask(
            verb_subject(K8sVerb::SshDetach, "lease-1"),
            &json!({"session_id": "s-1", "lease_id": "lease-1"}),
        )
        .await?;
    assert_eq!(detached["ok"], true, "{detached}");
    running.world.cluster.state().scripts.clear();
    running.world.cluster.script(ExecScript::new("grep -vF").exit(1));
    let detached = running
        .ask(
            verb_subject(K8sVerb::SshDetach, "lease-1"),
            &json!({"session_id": "s-2", "lease_id": "lease-1"}),
        )
        .await?;
    assert_eq!(detached["ok"], true, "{detached}");
    running.stop().await
}

#[tokio::test]
async fn terminals_and_domains_answer_for_a_held_lease() -> TestResult {
    let mut running = Running::start().await?;
    running.grant("lease-1").await?;
    let domain = running
        .verb(K8sVerb::GetDomain, "lease-1", json!({"port": 8080}))
        .await?;
    assert_eq!(
        typed::<GetSandboxDomainResponse>(&domain)?,
        GetSandboxDomainResponse::Unsupported
    );

    let closed = running
        .verb(
            K8sVerb::Terminal,
            "lease-1",
            json!({"terminal_id": "t-1", "op": "close"}),
        )
        .await?;
    assert!(
        matches!(typed(&closed)?, TerminalResponse::Done | TerminalResponse::Failed(_)),
        "{closed}"
    );
    running.stop().await
}

#[tokio::test]
async fn checkpoints_and_rollbacks_answer_for_a_held_lease() -> TestResult {
    let mut running = Running::start().await?;
    running.grant("lease-1").await?;
    let target = json!({"registry": running.world.registry.address, "repository": "snapshots/live", "artifact_type": "sandbox_rootfs_v1"});
    running
        .world
        .cluster
        .script(ExecScript::new("tar").stdout(b"archive-bytes"));
    let checkpoint = running
        .verb(K8sVerb::CheckpointRunning, "lease-1", json!({"target": target}))
        .await?;
    assert_eq!(checkpoint["outcome"], "ok", "{checkpoint}");
    let snapshot = seed_snapshot(&running.world.registry, "snapshots/seeded", b"content").await?;
    let rollback = running
        .verb(K8sVerb::Rollback, "lease-1", json!({"snapshot": snapshot}))
        .await?;
    assert!(rollback.get("outcome").is_some(), "{rollback}");
    running.world.registry.shutdown();
    let failed = running
        .verb(K8sVerb::CheckpointRunning, "lease-1", json!({"target": target}))
        .await?;
    assert_eq!(failed["outcome"], "err", "{failed}");
    running.stop().await
}
