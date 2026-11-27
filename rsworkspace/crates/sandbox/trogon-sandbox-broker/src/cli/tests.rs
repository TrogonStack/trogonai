use trogon_sandbox_contract::TenantId;

use std::error::Error;

use clap::Parser;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("trogon-sandbox-broker").chain(args.iter().copied()))
}

#[test]
fn auth_names_default_and_alternates_need_explicit_selection() -> TestResult {
    let default = parse(&[])?;
    assert_eq!(default.api_key_bucket.as_str(), "sandbox-api-keys");
    assert_eq!(default.spicedb_tenant_resource_type.as_str(), "sandbox_tenant");
    let custom = parse(&[
        "--api-key-bucket",
        "custom-api-keys",
        "--spicedb-tenant-resource-type",
        "custom_tenant",
    ])?;
    assert_eq!(custom.api_key_bucket.as_str(), "custom-api-keys");
    assert_eq!(custom.spicedb_tenant_resource_type.as_str(), "custom_tenant");
    assert!(parse(&["--api-key-bucket", "invalid.bucket"]).is_err());
    assert!(parse(&["--spicedb-tenant-resource-type", "Bad-Type"]).is_err());
    Ok(())
}

#[test]
fn enforcement_is_on_by_default() -> TestResult {
    let mut cli = parse(&[])?;
    assert!(!cli.apply_insecure_dev());
    assert!(cli.authz_enforce);
    assert!(cli.enforce_image_allowlist);
    assert!(cli.enforce_worker_registry);
    Ok(())
}

#[test]
fn insecure_dev_turns_all_three_off() -> TestResult {
    let mut cli = parse(&["--insecure-dev"])?;
    assert!(cli.apply_insecure_dev());
    assert!(!cli.authz_enforce);
    assert!(!cli.enforce_image_allowlist);
    assert!(!cli.enforce_worker_registry);
    Ok(())
}

#[test]
fn tenant_lists_are_parsed_into_tenant_ids() -> TestResult {
    let cli = parse(&[
        "--hardening-override-tenants",
        "acme,globex",
        "--native-passthrough-tenants",
        "acme",
        "--retention-sweep-tenants",
        "initech",
    ])?;
    let names =
        |tenants: &[TenantId]| -> Vec<String> { tenants.iter().map(|tenant| tenant.as_str().to_string()).collect() };
    assert_eq!(names(&cli.hardening_override_tenants), ["acme", "globex"]);
    assert_eq!(names(&cli.native_passthrough_tenants), ["acme"]);
    assert_eq!(names(&cli.retention_sweep_tenants), ["initech"]);
    assert!(parse(&["--hardening-override-tenants", "acme,"]).is_err());
    Ok(())
}

#[test]
fn defaults_leave_tenant_lists_empty() -> TestResult {
    let cli = parse(&[])?;
    assert!(cli.hardening_override_tenants.is_empty());
    assert!(cli.native_passthrough_tenants.is_empty());
    assert!(cli.retention_sweep_tenants.is_empty());
    Ok(())
}
