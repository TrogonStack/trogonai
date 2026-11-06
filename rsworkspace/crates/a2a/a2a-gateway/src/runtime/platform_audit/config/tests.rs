use trogon_std::env::InMemoryEnv;

use super::*;

fn env() -> InMemoryEnv {
    let env = InMemoryEnv::new();
    env.set(ENV_GATEWAY_AUDIT_TENANT, "acme");
    env.set(ENV_GATEWAY_AUDIT_PROJECT, "payments");
    env.set(ENV_GATEWAY_AUDIT_INSTANCE, "gw-1");
    env
}

#[test]
fn the_three_settings_name_the_tenant_project_and_writer() {
    let config = PlatformAuditConfig::from_env(&env()).expect("config");
    assert_eq!(config.identity.tenant().as_str(), "acme");
    assert_eq!(config.identity.project().as_str(), "payments");
    assert_eq!(config.identity.writer().as_str(), "a2a-gateway:gw-1");
    assert_eq!(config.identity.component().as_str(), "a2a_gateway");
}

#[test]
fn a_missing_setting_is_named() {
    for name in [
        ENV_GATEWAY_AUDIT_TENANT,
        ENV_GATEWAY_AUDIT_PROJECT,
        ENV_GATEWAY_AUDIT_INSTANCE,
    ] {
        let env = env();
        env.remove(name);
        assert_eq!(
            PlatformAuditConfig::from_env(&env),
            Err(PlatformAuditConfigError::Missing(name))
        );
    }
}

#[test]
fn a_blank_setting_counts_as_missing() {
    let env = env();
    env.set(ENV_GATEWAY_AUDIT_TENANT, "   ");
    assert_eq!(
        PlatformAuditConfig::from_env(&env),
        Err(PlatformAuditConfigError::Missing(ENV_GATEWAY_AUDIT_TENANT))
    );
}

#[test]
fn a_tenant_that_is_not_an_audit_name_is_rejected() {
    let env = env();
    env.set(ENV_GATEWAY_AUDIT_TENANT, "has space");
    assert!(matches!(
        PlatformAuditConfig::from_env(&env),
        Err(PlatformAuditConfigError::Tenant(_))
    ));
}

#[test]
fn a_project_that_is_not_a_subject_token_is_rejected() {
    let env = env();
    env.set(ENV_GATEWAY_AUDIT_PROJECT, "a.b");
    assert!(matches!(
        PlatformAuditConfig::from_env(&env),
        Err(PlatformAuditConfigError::Project(_))
    ));
}

#[test]
fn an_instance_that_makes_no_writer_id_is_rejected() {
    let env = env();
    env.set(ENV_GATEWAY_AUDIT_INSTANCE, "has space");
    assert!(matches!(
        PlatformAuditConfig::from_env(&env),
        Err(PlatformAuditConfigError::Instance(_))
    ));
}
