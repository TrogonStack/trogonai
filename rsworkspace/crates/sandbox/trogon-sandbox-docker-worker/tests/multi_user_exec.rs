//! Sandbox multi-agent users/groups: creating users, creating groups, and
//! running commands as a specific user via the docker worker's exec path.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker multi_user_exec
//! ```

mod docker_support;

use docker_support::{TestResult, build_provider, fresh_lease_id, lease_request_on_image};

use trogon_sandbox_contract::{
    BootMode, CommandEnv, CreateSandboxGroupResponse, CreateSandboxUserResponse, GroupMembershipAction, LeaseRequest,
    ModifyGroupMembershipResponse, SandboxGroupName, SandboxUserName, StreamedOutput,
};
use trogon_sandbox_protocol::Provider as _;

/// A local image built from plain Alpine plus the `shadow` and `bash` packages, so `useradd`, `groupadd`,
/// `usermod`, `gpasswd` and the `/bin/bash` login shell the worker requires are present without depending on
/// the sandboxed container's own network egress.
const ALPINE_SHADOW_IMAGE: &str = "trogon-sandbox-docker-worker-test-alpine-shadow:local";

/// Builds (or reuses a cached) local image with the `shadow` and `bash` packages installed, from a throwaway
/// build context.
fn build_alpine_shadow_image() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("Dockerfile"),
        "FROM alpine:3.20\nRUN apk add --no-cache shadow bash\n",
    )?;
    let status = std::process::Command::new("docker")
        .args(["build", "-t", ALPINE_SHADOW_IMAGE])
        .arg(dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .status()?;
    assert!(status.success(), "failed to build {ALPINE_SHADOW_IMAGE}");
    Ok(())
}

fn bare_lease_request() -> TestResult<LeaseRequest> {
    lease_request_on_image("multi-user-exec-test", BootMode::Bare, ALPINE_SHADOW_IMAGE)
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_user_exec_isolates_homes_and_shares_group_dir() -> TestResult {
    build_alpine_shadow_image()?;
    let provider = build_provider("multi_user_exec").await?;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let alice = SandboxUserName::new("alice")?;
    let bob = SandboxUserName::new("bob")?;

    let alice_created = provider.create_user_for_lease(&lease_id, &alice).await?;
    assert!(
        matches!(alice_created, CreateSandboxUserResponse::Created { ref home_dir } if home_dir == "/home/alice"),
        "expected Created for alice, got {alice_created:?}"
    );

    let bob_created = provider.create_user_for_lease(&lease_id, &bob).await?;
    assert!(
        matches!(bob_created, CreateSandboxUserResponse::Created { ref home_dir } if home_dir == "/home/bob"),
        "expected Created for bob, got {bob_created:?}"
    );

    // Re-creating alice should now report AlreadyExists rather than failing.
    let alice_again = provider.create_user_for_lease(&lease_id, &alice).await?;
    assert!(
        matches!(alice_again, CreateSandboxUserResponse::AlreadyExists { ref home_dir } if home_dir == "/home/alice"),
        "expected AlreadyExists on re-creation, got {alice_again:?}"
    );

    let whoami_alice = provider
        .run_command_for_lease(
            &lease_id,
            vec!["whoami".to_string()],
            None,
            &CommandEnv::default(),
            30,
            Some(&alice),
            StreamedOutput::Disabled,
            usize::MAX,
        )
        .await?;
    assert_eq!(String::from_utf8_lossy(&whoami_alice.stdout).trim(), "alice");

    let whoami_bob = provider
        .run_command_for_lease(
            &lease_id,
            vec!["whoami".to_string()],
            None,
            &CommandEnv::default(),
            30,
            Some(&bob),
            StreamedOutput::Disabled,
            usize::MAX,
        )
        .await?;
    assert_eq!(String::from_utf8_lossy(&whoami_bob.stdout).trim(), "bob");

    // alice's home directory is mode 770, owned by alice, so bob must not be
    // able to read into it.
    let bob_reads_alice_home = provider
        .run_command_for_lease(
            &lease_id,
            vec!["ls".to_string(), "/home/alice".to_string()],
            None,
            &CommandEnv::default(),
            30,
            Some(&bob),
            StreamedOutput::Disabled,
            usize::MAX,
        )
        .await?;
    assert_ne!(
        bob_reads_alice_home.exit_code, 0,
        "bob should not be able to list alice's home directory"
    );

    let group = SandboxGroupName::new("collab")?;
    let group_created = provider.create_group_for_lease(&lease_id, &group).await?;
    assert!(
        matches!(group_created, CreateSandboxGroupResponse::Created { ref shared_dir } if shared_dir == "/shared/collab"),
        "expected Created for collab group, got {group_created:?}"
    );

    for user in [&alice, &bob] {
        let applied = provider
            .modify_group_membership_for_lease(&lease_id, user, &group, GroupMembershipAction::Add)
            .await?;
        assert!(
            matches!(applied, ModifyGroupMembershipResponse::Applied),
            "expected Applied adding {user:?} to collab, got {applied:?}"
        );
    }

    let write_result = provider
        .run_command_for_lease(
            &lease_id,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo hello-from-alice > /shared/collab/note.txt".to_string(),
            ],
            None,
            &CommandEnv::default(),
            30,
            Some(&alice),
            StreamedOutput::Disabled,
            usize::MAX,
        )
        .await?;
    assert_eq!(
        write_result.exit_code,
        0,
        "stderr: {:?}",
        String::from_utf8_lossy(&write_result.stderr)
    );

    let read_result = provider
        .run_command_for_lease(
            &lease_id,
            vec!["cat".to_string(), "/shared/collab/note.txt".to_string()],
            None,
            &CommandEnv::default(),
            30,
            Some(&bob),
            StreamedOutput::Disabled,
            usize::MAX,
        )
        .await?;
    assert_eq!(
        read_result.exit_code,
        0,
        "stderr: {:?}",
        String::from_utf8_lossy(&read_result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&read_result.stdout).contains("hello-from-alice"),
        "stdout: {:?}",
        String::from_utf8_lossy(&read_result.stdout)
    );

    provider.cancel(&lease_id).await?;
    Ok(())
}
