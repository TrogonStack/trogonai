use trogon_peer_mcp::RuntimeConfig;
use trogon_peer_nats::PeerAccount;
use trogon_std::env::InMemoryEnv;

fn settings() -> InMemoryEnv {
    let env = InMemoryEnv::new();
    for (name, value) in [
        ("PEER_TENANT_ID", "tenant"),
        ("PEER_ACCOUNT_MODE", "tenant"),
        ("PEER_CALLER_ID", "caller"),
        ("PEER_CONNECTION_ID", "issued_connection"),
        ("PEER_INSTALLATION_ID", "persistent_installation"),
        ("PEER_COLLABORATION_REF", "collaboration"),
        ("PEER_DISPLAY_NAME", "reviewer"),
        ("PEER_AVAILABILITY", "ready"),
        ("NATS_TOKEN", "env://CONNECTOR_TEST_TOKEN"),
    ] {
        env.set(name, value);
    }
    env
}

#[test]
fn runtime_config_requires_explicit_typed_identity_and_never_debugs_settings() -> Result<(), Box<dyn std::error::Error>>
{
    let env = settings();
    let first = RuntimeConfig::from_env(&env)?;
    let second = RuntimeConfig::from_env(&env)?;
    assert!(matches!(first.requester().account(), PeerAccount::TenantAccount { .. }));
    assert_eq!(first.requester().account().tenant().as_str(), "tenant");
    assert_eq!(first.requester().caller().as_str(), "caller");
    assert_eq!(first.requester().connection().as_str(), "issued_connection");
    assert_ne!(
        first.connector_input().connector_id,
        second.connector_input().connector_id
    );
    assert_ne!(first.connector_input().connector_id, "issued_connection");
    assert_eq!(
        first.connector_input().installation_id,
        second.connector_input().installation_id
    );
    assert_eq!(format!("{first:?}"), "RuntimeConfig { .. }");
    for name in [
        "PEER_TENANT_ID",
        "PEER_ACCOUNT_MODE",
        "PEER_CALLER_ID",
        "PEER_CONNECTION_ID",
        "PEER_INSTALLATION_ID",
        "PEER_COLLABORATION_REF",
        "PEER_DISPLAY_NAME",
        "PEER_AVAILABILITY",
        "NATS_TOKEN",
    ] {
        let env = settings();
        env.remove(name);
        assert!(RuntimeConfig::from_env(&env).is_err(), "missing {name} admitted");
        env.set(name, "");
        assert!(RuntimeConfig::from_env(&env).is_err(), "empty {name} admitted");
    }
    Ok(())
}

#[test]
fn runtime_config_rejects_invalid_modes_tokens_observations_and_anonymous_auth()
-> Result<(), Box<dyn std::error::Error>> {
    let env = settings();
    env.set("PEER_ACCOUNT_MODE", "shared_fixture");
    assert!(matches!(
        RuntimeConfig::from_env(&env)?.requester().account(),
        PeerAccount::SharedFixture { .. }
    ));
    for (name, value) in [
        ("PEER_ACCOUNT_MODE", "automatic"),
        ("PEER_TENANT_ID", "tenant.*"),
        ("PEER_CALLER_ID", "caller.other"),
        ("PEER_CONNECTION_ID", ">"),
        ("PEER_INSTALLATION_ID", "bad\nidentity"),
        ("PEER_AVAILABILITY", "active"),
    ] {
        let env = settings();
        env.set(name, value);
        assert!(RuntimeConfig::from_env(&env).is_err(), "invalid {name} admitted");
    }
    Ok(())
}

#[test]
fn runtime_configuration_preserves_broker_order_tls_and_unresolved_auth_for_each_observation()
-> Result<(), Box<dyn std::error::Error>> {
    use trogon_nats::{CredentialValue, NatsAuth, NatsTls};
    use trogon_peer_mcp::AvailabilityObservation;

    for (setting, observation) in [
        ("ready", AvailabilityObservation::Ready),
        ("busy", AvailabilityObservation::Busy),
        ("blocked", AvailabilityObservation::Blocked),
        ("draining", AvailabilityObservation::Draining),
    ] {
        let env = settings();
        env.set("PEER_AVAILABILITY", setting);
        env.set("NATS_URL", "tls://first.invalid:4222, tls://second.invalid:4222");
        let config = RuntimeConfig::from_env(&env)?;
        assert_eq!(
            std::mem::discriminant(&config.availability()),
            std::mem::discriminant(&observation)
        );
        assert_eq!(
            config.nats().servers,
            ["tls://first.invalid:4222", "tls://second.invalid:4222"]
        );
        assert!(matches!(config.nats().tls, Ok(NatsTls::Enabled { .. })));
        assert!(matches!(
            &config.nats().auth,
            NatsAuth::Token(CredentialValue::Env(name)) if name == "CONNECTOR_TEST_TOKEN"
        ));
        assert_eq!(format!("{config:?}"), "RuntimeConfig { .. }");
    }
    Ok(())
}
