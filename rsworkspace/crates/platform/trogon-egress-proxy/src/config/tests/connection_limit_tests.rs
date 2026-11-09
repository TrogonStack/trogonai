use super::*;
use crate::connection_limit::{ConnectionLimit, ConnectionLimitError};

#[test]
fn the_cli_connection_limit_controls_the_validated_runtime_budget() {
    let config = resolve(parse(&["--max-connections", "2"]).expect("arguments")).expect("configuration");
    assert_eq!(config.limits.max_connections, ConnectionLimit::new(2).expect("limit"));
    let default = resolve(parse(&[]).expect("arguments")).expect("configuration");
    assert_eq!(default.limits.max_connections, ConnectionLimit::default());
}

#[test]
fn invalid_connection_limits_are_typed_startup_errors() {
    for (limit, error) in [
        (0, ConnectionLimitError::Zero),
        (4097, ConnectionLimitError::TooLarge),
        (usize::MAX, ConnectionLimitError::TooLarge),
    ] {
        let mut args = parse(&[]).expect("arguments");
        args.max_connections = limit;
        assert_eq!(
            resolve(args).expect_err("invalid connection limit"),
            ConfigError::ConnectionLimit(error)
        );
    }
}
