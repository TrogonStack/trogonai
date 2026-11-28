use trogon_std::env::InMemoryEnv;

use super::*;

type TestResult = Result<(), FuseConfigError>;

#[test]
fn unset_variables_keep_fuse_disabled() -> TestResult {
    let config = FuseConfig::from_env(&InMemoryEnv::new())?;
    assert!(!config.enabled);
    assert!(!config.allow_other);
    Ok(())
}

#[test]
fn recognised_flags_parse() -> TestResult {
    let env = InMemoryEnv::new();
    env.set("SANDBOX_FUSE_ENABLED", "TRUE");
    env.set("SANDBOX_FUSE_ALLOW_OTHER", "0");
    let config = FuseConfig::from_env(&env)?;
    assert!(config.enabled);
    assert!(!config.allow_other);
    Ok(())
}

#[test]
fn an_unrecognised_flag_is_refused() {
    for name in ["SANDBOX_FUSE_ENABLED", "SANDBOX_FUSE_ALLOW_OTHER"] {
        let env = InMemoryEnv::new();
        env.set(name, "yes");
        assert!(matches!(
            FuseConfig::from_env(&env),
            Err(FuseConfigError::NotAFlag { name: refused }) if refused == name
        ));
    }
}
