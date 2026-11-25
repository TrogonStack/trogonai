use clap::Parser;
use clap::error::ErrorKind;

use super::*;
use crate::ConfigCommand;

fn parsed(args: &[&str]) -> ServiceArgs {
    ServiceArgs::try_parse_from(args).expect("command line parses")
}

fn dump() -> Option<ServiceCommand> {
    Some(ServiceCommand::Config {
        command: ConfigCommand::Dump { resolved: true },
    })
}

#[test]
fn a_config_dump_without_resolved_is_refused() {
    let error = ServiceArgs::try_parse_from(["trogon-secrets-service", "config", "dump"])
        .expect_err("config dump names what it dumps");

    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
}

#[test]
fn an_unknown_command_is_refused() {
    let error = ServiceArgs::try_parse_from(["trogon-secrets-service", "config", "show"])
        .expect_err("config has no show command");

    assert_eq!(error.kind(), ErrorKind::InvalidSubcommand);
}

#[test]
fn no_command_serves_releases() {
    let args = parsed(&["trogon-secrets-service", "--config", "/etc/trogon/secrets-service.toml"]);

    assert_eq!(args.command, None);
    assert_eq!(
        args.runtime.config.as_deref(),
        Some(std::path::Path::new("/etc/trogon/secrets-service.toml"))
    );
}

#[test]
fn config_dump_resolved_asks_for_the_dump() {
    assert_eq!(
        parsed(&["trogon-secrets-service", "config", "dump", "--resolved"]).command,
        dump()
    );
}

#[test]
fn the_configuration_flags_apply_to_the_dump_wherever_they_are_given() {
    let before = parsed(&[
        "trogon-secrets-service",
        "--config",
        "/etc/trogon/secrets-service.toml",
        "--nats-url",
        "nats://nats.internal:4222",
        "--invalidation-stream-replicas",
        "5",
        "config",
        "dump",
        "--resolved",
    ]);
    let after = parsed(&[
        "trogon-secrets-service",
        "config",
        "dump",
        "--resolved",
        "--config",
        "/etc/trogon/secrets-service.toml",
        "--nats-url",
        "nats://nats.internal:4222",
        "--invalidation-stream-replicas",
        "5",
    ]);

    for args in [before, after] {
        assert_eq!(args.command, dump());
        assert_eq!(
            args.runtime.config.as_deref(),
            Some(std::path::Path::new("/etc/trogon/secrets-service.toml"))
        );
        assert_eq!(args.runtime.nats.nats_url.as_deref(), Some("nats://nats.internal:4222"));
        assert_eq!(args.invalidation_stream_replicas, Some(5));
    }
}
