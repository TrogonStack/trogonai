use super::*;

#[test]
fn states_parse_from_their_snake_case_names() -> Result<(), serde_json::Error> {
    let started: SandboxState = serde_json::from_str("\"started\"")?;
    assert_eq!(started, SandboxState::Started);
    let failed: SandboxState = serde_json::from_str("\"build_failed\"")?;
    assert_eq!(failed, SandboxState::BuildFailed);
    let unknown: SandboxState = serde_json::from_str("\"somethingnew\"")?;
    assert_eq!(unknown, SandboxState::Unknown);
    Ok(())
}

#[test]
fn only_dead_end_states_are_failed() {
    for state in [
        SandboxState::Error,
        SandboxState::BuildFailed,
        SandboxState::Destroyed,
        SandboxState::Destroying,
    ] {
        assert!(state.is_failed(), "{state:?}");
    }
    for state in [
        SandboxState::Started,
        SandboxState::Stopped,
        SandboxState::Starting,
        SandboxState::Unknown,
    ] {
        assert!(!state.is_failed(), "{state:?}");
    }
}
