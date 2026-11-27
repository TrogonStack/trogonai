use std::error::Error as _;

use trogon_sandbox_guest_init::{
    GuestActor, GuestActorId, GuestActorIdError, GuestAgentPort, GuestAgentPortError, GuestBootConfig,
    GuestBootConfigError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn reads_actor_and_port() -> TestResult {
    let line = "console=ttyS0 ate.actor_id=actor-7 sandbox.agent_port=9000 quiet\n";
    let config = GuestBootConfig::from_kernel_command_line(line)?;
    assert_eq!(config.actor(), &GuestActor::Bound(GuestActorId::new("actor-7")?));
    assert_eq!(config.actor().as_str(), "actor-7");
    assert_eq!(config.agent_port().get(), 9000);
    assert_eq!(config.agent_port().to_string(), "9000");
    Ok(())
}

#[test]
fn defaults_when_arguments_are_absent() -> TestResult {
    let config = GuestBootConfig::from_kernel_command_line("console=ttyS0")?;
    assert_eq!(config.actor(), &GuestActor::Unbound);
    assert_eq!(config.actor().as_str(), "unbound");
    assert_eq!(config.agent_port(), GuestAgentPort::DEFAULT);
    assert_eq!(config.agent_port().get(), 8110);
    Ok(())
}

#[test]
fn unbound_marker_means_unbound() -> TestResult {
    let config = GuestBootConfig::from_kernel_command_line("ate.actor_id=unbound")?;
    assert_eq!(config.actor(), &GuestActor::Unbound);
    Ok(())
}

#[test]
fn first_occurrence_wins() -> TestResult {
    let line = "ate.actor_id=a sandbox.agent_port=1 ate.actor_id=b sandbox.agent_port=2";
    let config = GuestBootConfig::from_kernel_command_line(line)?;
    assert_eq!(config.actor().as_str(), "a");
    assert_eq!(config.agent_port().get(), 1);
    Ok(())
}

#[test]
fn rejects_empty_actor_id() {
    assert_eq!(
        GuestBootConfig::from_kernel_command_line("ate.actor_id= quiet"),
        Err(GuestBootConfigError::Actor(GuestActorIdError::Empty))
    );
}

#[test]
fn actor_id_is_never_empty_or_the_unbound_marker() -> TestResult {
    assert_eq!(GuestActorId::new(""), Err(GuestActorIdError::Empty));
    assert_eq!(GuestActorId::new("unbound"), Err(GuestActorIdError::UnboundMarker));
    let actor_id = GuestActorId::new("actor-7")?;
    assert_eq!(actor_id.as_str(), "actor-7");
    assert_eq!(actor_id.to_string(), "actor-7");
    Ok(())
}

#[test]
fn rejects_zero_port() {
    assert_eq!(
        GuestBootConfig::from_kernel_command_line("sandbox.agent_port=0"),
        Err(GuestBootConfigError::AgentPort(GuestAgentPortError::Zero))
    );
    assert_eq!(GuestAgentPort::new(0), Err(GuestAgentPortError::Zero));
}

#[test]
fn rejects_non_numeric_and_out_of_range_ports() {
    for raw in ["abc", "", "-1", "4294967296", "+"] {
        let line = format!("sandbox.agent_port={raw}");
        let error = GuestBootConfig::from_kernel_command_line(&line);
        assert!(
            matches!(
                &error,
                Err(GuestBootConfigError::AgentPort(GuestAgentPortError::NotANumber { value, .. })) if value == raw
            ),
            "{raw}: {error:?}"
        );
    }
}

#[test]
fn port_parse_error_keeps_its_source() {
    let error = "4294967296".parse::<GuestAgentPort>();
    assert!(matches!(&error, Err(GuestAgentPortError::NotANumber { .. })));
    assert!(
        error
            .err()
            .and_then(|error| error.source().map(ToString::to_string))
            .is_some()
    );
}

#[test]
fn error_messages_name_the_problem() {
    assert_eq!(
        GuestAgentPortError::Zero.to_string(),
        "guest agent port must be nonzero"
    );
    assert!(matches!(
        "x".parse::<GuestAgentPort>(),
        Err(GuestAgentPortError::NotANumber { value, .. }) if value == "x"
    ));
    assert_eq!(GuestActorIdError::Empty.to_string(), "guest actor id must not be empty");
    assert_eq!(
        GuestBootConfigError::Actor(GuestActorIdError::UnboundMarker).to_string(),
        "guest actor id must not be the unbound marker"
    );
    assert_eq!(
        GuestBootConfigError::AgentPort(GuestAgentPortError::Zero).to_string(),
        "guest agent port must be nonzero"
    );
}
