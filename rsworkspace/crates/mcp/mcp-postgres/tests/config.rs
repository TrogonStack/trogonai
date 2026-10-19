use mcp_postgres::{
    ConfigInput, ConnectionId, DatabaseOptions, EnvironmentVariable, ObjectId, RuntimeConfig, SpiceDbEndpoint,
};
#[test]
fn rejects_duplicate_connection_names_and_ids() -> Result<(), Box<dyn std::error::Error>> {
    for connections in [
        "[{name='a',id='one',dsn_env='DSN'},{name='a',id='two',dsn_env='DSN'}]",
        "[{name='a',id='one',dsn_env='DSN'},{name='b',id='one',dsn_env='DSN'}]",
    ] {
        let input: ConfigInput = toml::from_str(&format!(
            "user='alice'\nclient='desktop'\nspicedb_endpoint='http://localhost:50051'\nspicedb_token_env='TOKEN'\nconnections={connections}"
        ))?;
        assert!(
            RuntimeConfig::from_input(input, |key| Some(
                if key == "DSN" {
                    "postgres://reader@localhost/example"
                } else {
                    "token"
                }
                .into()
            ))
            .is_err()
        );
    }
    Ok(())
}
#[test]
fn unknown_settings_fail_closed() {
    let valid = include_str!("../examples/config.toml");
    assert!(toml::from_str::<ConfigInput>(valid).is_ok());
    assert!(toml::from_str::<ConfigInput>(&format!("permissions_disabled=true\n{valid}")).is_err());
}

#[test]
fn secrets_and_remote_transport_settings_are_validated_at_the_boundary() -> Result<(), Box<dyn std::error::Error>> {
    assert!(DatabaseOptions::new("postgres://reader@localhost/example").is_ok());
    assert!(DatabaseOptions::new("postgres://reader@db.example.test/example?sslmode=verify-full").is_ok());
    assert!(DatabaseOptions::new("postgres://reader@db.example.test/example?sslmode=disable").is_err());
    assert!(SpiceDbEndpoint::new("https://policy.example.test:443".into()).is_ok());
    assert!(SpiceDbEndpoint::new("http://policy.example.test:50051".into()).is_err());
    assert!(SpiceDbEndpoint::new("https://user:password@policy.example.test".into()).is_err());
    assert!(SpiceDbEndpoint::new("https://policy.example.test/path".into()).is_err());
    assert!(EnvironmentVariable::new("MCP_POSTGRES_DSN".into()).is_ok());
    assert!(EnvironmentVariable::new("not-an-env-reference".into()).is_err());
    assert!(ObjectId::new(String::new()).is_err());
    assert!(ObjectId::new("untrusted:*".into()).is_err());

    let input: ConfigInput = toml::from_str(include_str!("../examples/config.toml"))?;
    assert!(RuntimeConfig::from_input(input, |_| None).is_err());
    Ok(())
}

#[test]
fn authorization_ids_reject_dots_at_configuration_boundary() -> Result<(), Box<dyn std::error::Error>> {
    assert!(ObjectId::new("invalid.id".into()).is_err());
    for field in ["user", "client", "connection"] {
        let mut input: ConfigInput = toml::from_str(include_str!("../examples/config.toml"))?;
        match field {
            "user" => input.user = "invalid.id".into(),
            "client" => input.client = "invalid.id".into(),
            _ => input.connections[0].id = "invalid.id".into(),
        }
        assert!(RuntimeConfig::from_input(input, config_environment).is_err(), "{field}");
    }
    Ok(())
}

#[test]
fn connection_ids_reserve_room_for_catalog_resource_suffixes() -> Result<(), Box<dyn std::error::Error>> {
    let suffix = format!("/{}/{}/{}", u32::MAX, u32::MAX, i16::MAX);
    let maximum = 200 - suffix.len();
    let id = ConnectionId::new("a".repeat(maximum))?;
    assert_eq!(id.as_str().len(), maximum);
    assert_eq!(id.into_object_id(), ObjectId::new("a".repeat(maximum))?);
    assert!(ConnectionId::new("a".repeat(maximum + 1)).is_err());
    assert!(ConnectionId::new("invalid.id".into()).is_err());
    let mut input: ConfigInput = toml::from_str(include_str!("../examples/config.toml"))?;
    input.connections[0].id = "a".repeat(maximum);
    assert!(RuntimeConfig::from_input(input, config_environment).is_ok());
    assert!(ObjectId::new(format!("{}{suffix}", "a".repeat(maximum))).is_ok());
    let mut input: ConfigInput = toml::from_str(include_str!("../examples/config.toml"))?;
    input.connections[0].id = "a".repeat(maximum + 1);
    assert!(RuntimeConfig::from_input(input, config_environment).is_err());
    assert!(ObjectId::new("a".repeat(200)).is_ok());
    Ok(())
}

fn config_environment(key: &str) -> Option<String> {
    Some(
        if key.contains("DSN") {
            "postgres://reader@localhost/example"
        } else {
            "token"
        }
        .into(),
    )
}
