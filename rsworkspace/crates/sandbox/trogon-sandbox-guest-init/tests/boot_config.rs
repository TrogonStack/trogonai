use trogon_sandbox_guest_init::{GuestAgentPort, GuestAgentPortError, GuestBootConfig};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn reads_actor_and_port() -> TestResult {
    let config = GuestBootConfig::from_kernel_command_line(
        "console=ttyS0 ate.actor_id=actor-7 sandbox.agent_port=9000 quiet\n",
    )?;
    assert_eq!(config.actor_id(), "actor-7");
    assert_eq!(config.agent_port().get(), 9000);
    assert_eq!(config.agent_port().to_string(), "9000");
    Ok(())
}

#[test]
fn defaults_when_arguments_are_absent() -> TestResult {
    let config = GuestBootConfig::from_kernel_command_line("console=ttyS0")?;
    assert_eq!(config.actor_id(), "unbound");
    assert_eq!(config.agent_port(), GuestAgentPort::DEFAULT);
    assert_eq!(config.agent_port().get(), 8110);
    Ok(())
}

#[test]
fn first_occurrence_wins() -> TestResult {
    let config = GuestBootConfig::from_kernel_command_line(
        "ate.actor_id=a sandbox.agent_port=1 ate.actor_id=b sandbox.agent_port=2",
    )?;
    assert_eq!(config.actor_id(), "a");
    assert_eq!(config.agent_port().get(), 1);
    Ok(())
}

#[test]
fn rejects_zero_port() {
    assert_eq!(
        GuestBootConfig::from_kernel_command_line("sandbox.agent_port=0"),
        Err(GuestAgentPortError::Zero)
    );
    assert_eq!(GuestAgentPort::new(0), Err(GuestAgentPortError::Zero));
}

#[test]
fn rejects_non_numeric_and_out_of_range_ports() {
    for raw in ["abc", "", "-1", "4294967296", "+"] {
        let line = format!("sandbox.agent_port={raw}");
        assert_eq!(
            GuestBootConfig::from_kernel_command_line(&line),
            Err(GuestAgentPortError::NotANumber(raw.to_owned()))
        );
    }
}

#[test]
fn error_messages_name_the_problem() {
    assert_eq!(
        GuestAgentPortError::Zero.to_string(),
        "guest agent port must be nonzero"
    );
    assert!(GuestAgentPortError::NotANumber("x".into()).to_string().contains("'x'"));
}
