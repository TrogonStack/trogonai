use mcp_postgres::{ConfigInput, DatabaseOptions, EnvironmentVariable, ObjectId, RuntimeConfig, SpiceDbEndpoint};
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
