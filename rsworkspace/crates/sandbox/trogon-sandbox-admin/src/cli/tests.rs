use clap::Parser;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn mint_args(tenant: &str) -> Vec<String> {
    [
        "trogon-sandbox-admin",
        "api-key",
        "mint",
        "--tenant",
        tenant,
        "--service-account",
        "sa",
        "--name",
        "key",
    ]
    .map(String::from)
    .to_vec()
}

fn freeze_args(verb: &str, extra: &[&str]) -> Vec<String> {
    ["trogon-sandbox-admin", "lease", verb, "lease-1"]
        .into_iter()
        .chain(extra.iter().copied())
        .map(String::from)
        .collect()
}

fn lease_command(args: Vec<String>) -> Result<LeaseCommand, Box<dyn std::error::Error>> {
    match Cli::try_parse_from(args)?.command {
        Command::Lease(command) => Ok(command),
        _ => Err("expected a lease command".into()),
    }
}

#[test]
fn lease_freeze_verbs_parse_with_and_without_force() -> TestResult {
    for verb in ["clear-freeze", "retry-freeze"] {
        let (LeaseCommand::ClearFreeze(args) | LeaseCommand::RetryFreeze(args)) =
            lease_command(freeze_args(verb, &[]))?;
        assert_eq!(args.lease_id.to_string(), "lease-1");
        assert!(!args.force, "{verb}");
        assert_eq!(args.active_lease_bucket, "sandbox-active-leases");
        assert_eq!(args.idempotency_bucket, "sandbox-idempotency");

        let (LeaseCommand::ClearFreeze(args) | LeaseCommand::RetryFreeze(args)) =
            lease_command(freeze_args(verb, &["--force"]))?;
        assert!(args.force, "{verb}");
    }
    Ok(())
}

#[test]
fn lease_freeze_verbs_map_to_their_variant() -> TestResult {
    assert!(matches!(
        lease_command(freeze_args("clear-freeze", &[]))?,
        LeaseCommand::ClearFreeze(_)
    ));
    assert!(matches!(
        lease_command(freeze_args("retry-freeze", &[]))?,
        LeaseCommand::RetryFreeze(_)
    ));
    Ok(())
}

#[test]
fn lease_freeze_verbs_need_a_lease_id() {
    for verb in ["clear-freeze", "retry-freeze"] {
        assert!(Cli::try_parse_from(["trogon-sandbox-admin", "lease", verb]).is_err());
    }
}

#[test]
fn mint_rejects_a_malformed_tenant() {
    for tenant in ["", "acme\tteam", &"a".repeat(129)] {
        assert!(Cli::try_parse_from(mint_args(tenant)).is_err(), "{tenant:?}");
    }
}

#[test]
fn mint_accepts_tenants_with_separators() {
    for tenant in ["acme", "org:acme", "org/acme"] {
        assert!(Cli::try_parse_from(mint_args(tenant)).is_ok(), "{tenant:?}");
    }
}

#[test]
fn worker_verbs_parse() -> TestResult {
    for verb in ["cordon", "drain", "uncordon"] {
        let cli = Cli::try_parse_from([
            "trogon-sandbox-admin",
            "worker",
            verb,
            "--provider",
            "docker",
            "--worker",
            "w1",
            "--reason",
            "kernel patch",
        ])?;
        let Command::Worker(WorkerCommand::Cordon(args) | WorkerCommand::Drain(args) | WorkerCommand::Uncordon(args)) =
            cli.command
        else {
            return Err(format!("{verb} must parse as a fence verb").into());
        };
        assert_eq!(args.target.worker.to_string(), "w1");
        assert_eq!(args.reason.as_deref(), Some("kernel patch"));
    }
    assert!(Cli::try_parse_from(["trogon-sandbox-admin", "worker", "cordon"]).is_err());
    assert!(Cli::try_parse_from(["trogon-sandbox-admin", "worker", "list"]).is_ok());
    Ok(())
}

#[test]
fn worker_rejects_an_unknown_provider() {
    assert!(
        Cli::try_parse_from([
            "trogon-sandbox-admin",
            "worker",
            "status",
            "--provider",
            "not-a-provider",
            "--worker",
            "w1",
        ])
        .is_err()
    );
}

#[test]
fn api_key_commands_select_a_validated_bucket() -> TestResult {
    let default = Cli::try_parse_from(["trogon-sandbox-admin", "api-key", "list"])?;
    assert_eq!(default.api_key_bucket.as_str(), "sandbox-api-keys");
    let custom = Cli::try_parse_from([
        "trogon-sandbox-admin",
        "api-key",
        "list",
        "--api-key-bucket",
        "custom-keys",
    ])?;
    assert_eq!(custom.api_key_bucket.as_str(), "custom-keys");
    assert!(
        Cli::try_parse_from([
            "trogon-sandbox-admin",
            "api-key",
            "list",
            "--api-key-bucket",
            "bad bucket!"
        ])
        .is_err()
    );
    Ok(())
}

#[test]
fn tenant_arguments_are_validated() {
    assert!(Cli::try_parse_from(["trogon-sandbox-admin", "image-policy", "show", "--tenant", "acme"]).is_ok());
    assert!(Cli::try_parse_from(["trogon-sandbox-admin", "image-policy", "show", "--tenant", ""]).is_err());
}

#[test]
fn image_policy_patterns_are_validated() {
    let parse = |pattern: &str| {
        Cli::try_parse_from([
            "trogon-sandbox-admin",
            "image-policy",
            "set",
            "--tenant",
            "acme",
            "--pattern",
            pattern,
        ])
    };
    assert!(parse("ghcr.io/acme/*").is_ok());
    assert!(parse("a/*/c").is_err());
}

#[test]
fn credential_provider_all_selects_every_provider() -> TestResult {
    let selection: ProviderSelection = "all".parse()?;
    assert!(selection.0.is_none());
    let selection: ProviderSelection = "docker".parse()?;
    assert!(selection.0.is_some());
    assert!("nonsense".parse::<ProviderSelection>().is_err());
    Ok(())
}

#[test]
fn tenant_limit_per_tag_flags_need_each_other() {
    let base = [
        "trogon-sandbox-admin",
        "tenant-limit",
        "set",
        "--tenant",
        "acme",
        "--max-cpu-seconds",
        "10",
        "--max-memory-mib",
        "512",
    ];
    let with = |extra: &[&str]| Cli::try_parse_from(base.iter().chain(extra.iter()).copied());
    assert!(with(&["--per-tag-key", "team"]).is_err());
    assert!(with(&["--per-tag-max", "3"]).is_err());
    assert!(with(&["--per-tag-key", "team", "--per-tag-max", "3"]).is_ok());
}
