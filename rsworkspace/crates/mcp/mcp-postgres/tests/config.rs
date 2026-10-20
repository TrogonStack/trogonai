use mcp_postgres::{
    ConfigInput, ConnectionId, DatabaseOptions, EnvironmentVariable, Error, Identifier, ObjectId, Parameter,
    ParameterInput, RuntimeConfig, SpiceDbEndpoint,
};
#[test]
fn rejects_duplicate_connection_names_and_ids() -> Result<(), Box<dyn std::error::Error>> {
    for connections in [
        "[{name='a',id='one',dsn_env='DSN'},{name='a',id='two',dsn_env='DSN'}]",
        "[{name='a',id='one',dsn_env='DSN'},{name='b',id='one',dsn_env='DSN'}]",
    ] {
        let input: ConfigInput = toml::from_str(&format!(
            "spicedb_endpoint='http://localhost:50051'\nspicedb_token_env='TOKEN'\nconnections={connections}"
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
    for field in ["user", "client"] {
        assert!(toml::from_str::<ConfigInput>(&format!("{field}=\"legacy-identity\"\n{valid}")).is_err());
    }
    assert!(toml::from_str::<ConfigInput>(&format!("permissions_disabled=true\n{valid}")).is_err());
}

#[test]
fn remote_postgres_connections_require_full_certificate_verification() {
    for mode in [
        "",
        "?sslmode=disable",
        "?sslmode=allow",
        "?sslmode=prefer",
        "?sslmode=require",
        "?sslmode=verify-ca",
    ] {
        assert!(
            matches!(
                DatabaseOptions::new(&format!("postgres://reader@db.example.test/example{mode}")),
                Err(Error::Configuration)
            ),
            "rejected TLS mode: {mode}"
        );
    }
    assert!(DatabaseOptions::new("postgres://reader@db.example.test/example?sslmode=verify-full").is_ok());
}

#[test]
fn loopback_postgres_connections_allow_local_plaintext_fixtures() {
    for host in ["localhost", "127.0.0.1", "[::1]"] {
        for mode in ["", "?sslmode=disable", "?sslmode=verify-full"] {
            assert!(
                DatabaseOptions::new(&format!("postgres://reader@{host}/example{mode}")).is_ok(),
                "host={host}, mode={mode}"
            );
        }
    }
    for host in ["localhost.example.test", "127.0.0.2", "db.example.test"] {
        assert!(matches!(
            DatabaseOptions::new(&format!("postgres://reader@{host}/example?sslmode=disable")),
            Err(Error::Configuration)
        ));
    }
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
    let mut input: ConfigInput = toml::from_str(include_str!("../examples/config.toml"))?;
    input.connections[0].id = "invalid.id".into();
    assert!(RuntimeConfig::from_input(input, config_environment).is_err());
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

#[test]
fn runtime_configuration_requires_connections_and_nonempty_secrets() -> Result<(), Box<dyn std::error::Error>> {
    let valid = include_str!("../examples/config.toml");
    let config = RuntimeConfig::from_input(toml::from_str(valid)?, config_environment)?;
    assert_eq!(config.spicedb_endpoint(), "http://127.0.0.1:50051");
    assert_eq!(config.spicedb_token(), "token");

    let mut empty: ConfigInput = toml::from_str(valid)?;
    empty.connections.clear();
    assert!(matches!(
        RuntimeConfig::from_input(empty, config_environment),
        Err(Error::Configuration)
    ));
    for missing_token in [false, true] {
        let input: ConfigInput = toml::from_str(valid)?;
        let token_key = input.spicedb_token_env.clone();
        assert!(matches!(
            RuntimeConfig::from_input(input, |key| {
                if key == token_key {
                    if missing_token { None } else { Some(String::new()) }
                } else {
                    config_environment(key)
                }
            }),
            Err(Error::Configuration)
        ));
    }
    assert!(matches!(
        RuntimeConfig::from_input(toml::from_str(valid)?, |_| Some(String::new())),
        Err(Error::Configuration)
    ));
    Ok(())
}

#[test]
fn identifiers_and_parameters_enforce_postgres_and_request_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    for invalid in [String::new(), "x".repeat(64), "contains\0nul".into(), "é".repeat(32)] {
        assert!(matches!(Identifier::new(invalid), Err(Error::Identifier)));
    }
    assert_eq!(Identifier::new("x".repeat(63))?.as_str().len(), 63);
    assert_eq!(Identifier::new("odd\"name".into())?.quoted(), "\"odd\"\"name\"");
    for input in [
        ParameterInput::Boolean(false),
        ParameterInput::Integer(i64::MIN),
        ParameterInput::Text("x".repeat(16_384)),
    ] {
        Parameter::try_from(input)?;
    }
    for input in [
        ParameterInput::Text("x".repeat(16_385)),
        ParameterInput::Text("a\0b".into()),
        ParameterInput::Text("é".repeat(8193)),
    ] {
        assert!(matches!(Parameter::try_from(input), Err(Error::Limit)));
    }
    Ok(())
}
