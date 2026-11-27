use crate::PROFILE;

#[test]
fn terminals_are_advertised_wherever_a_backend_can_attach_a_pty() {
    assert_eq!(
        PROFILE.isolation.supports_terminal.is_supported(),
        cfg!(any(target_os = "linux", target_os = "macos"))
    );
}
