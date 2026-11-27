use super::*;

#[test]
fn default_options_fit_every_backend() -> Result<(), Box<dyn std::error::Error>> {
    ensure_enforceable(&PlatformOptions::default(), false, false)?;
    Ok(())
}

#[test]
fn an_option_for_a_platform_the_backend_is_not_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = PlatformOptions::default();
    options.macos.allow_pty = true.into();
    assert!(ensure_enforceable(&options, false, true).is_err());
    ensure_enforceable(&options, true, false)?;

    let mut options = PlatformOptions::default();
    options.linux.weaker_nested_sandbox = true.into();
    assert!(ensure_enforceable(&options, true, false).is_err());
    ensure_enforceable(&options, false, true)?;
    Ok(())
}

#[test]
fn a_filesystem_option_needs_a_backend_that_enforces_rules() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = PlatformOptions::default();
    options.filesystem.allow_git_config = true.into();
    assert!(ensure_enforceable(&options, false, false).is_err());
    ensure_enforceable(&options, true, false)?;
    ensure_enforceable(&options, false, true)?;
    Ok(())
}
