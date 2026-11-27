use trogon_sandbox_contract::{SandboxGroupName, SandboxUserName};
use trogon_sandbox_protocol::{home_dir_for_user, shared_dir_for_group};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn home_dir_uses_the_home_prefix() -> TestResult {
    let username = SandboxUserName::new("agent-a")?;
    assert_eq!(home_dir_for_user(&username), "/home/agent-a");
    Ok(())
}

#[test]
fn shared_dir_uses_the_shared_prefix() -> TestResult {
    let groupname = SandboxGroupName::new("agents")?;
    assert_eq!(shared_dir_for_group(&groupname), "/shared/agents");
    Ok(())
}
