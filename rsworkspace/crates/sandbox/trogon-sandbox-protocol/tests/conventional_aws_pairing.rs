mod common;

use common::{TestResult, binding, bindings, mask_binding, name, pair};
use trogon_sandbox_contract::{AwsCredentialPairs, Extract, ExtractPattern, SecretBindings, SecretMode};
use trogon_sandbox_protocol::{
    CONVENTIONAL_AWS_ACCESS_KEY_ID, CONVENTIONAL_AWS_SECRET_ACCESS_KEY, CONVENTIONAL_AWS_SESSION_TOKEN, UnpairedReason,
    conventional_aws_pair,
};

const HOSTS: &[&str] = &["aws.example.com"];

fn no_declared() -> AwsCredentialPairs {
    AwsCredentialPairs::default()
}

fn trio(list: &[(&str, &str, &[&str])]) -> Result<SecretBindings, Box<dyn std::error::Error>> {
    let built = list
        .iter()
        .map(|(secret, env, hosts)| mask_binding(secret, env, hosts))
        .collect::<Result<Vec<_>, _>>()?;
    bindings(built)
}

#[test]
fn the_trio_pairs() -> TestResult {
    let set = trio(&[
        ("id", CONVENTIONAL_AWS_ACCESS_KEY_ID, HOSTS),
        ("secret", CONVENTIONAL_AWS_SECRET_ACCESS_KEY, HOSTS),
        ("token", CONVENTIONAL_AWS_SESSION_TOKEN, HOSTS),
    ])?;
    let result = conventional_aws_pair(&set, &no_declared());
    let paired = result.pair.ok_or("expected a pair")?;
    assert_eq!(paired.access_key_id(), &name("id")?);
    assert_eq!(paired.secret_access_key(), &name("secret")?);
    assert_eq!(paired.session_token(), Some(&name("token")?));
    assert!(result.unpaired.is_empty());
    Ok(())
}

#[test]
fn the_session_token_is_optional() -> TestResult {
    let set = trio(&[
        ("id", CONVENTIONAL_AWS_ACCESS_KEY_ID, HOSTS),
        ("secret", CONVENTIONAL_AWS_SECRET_ACCESS_KEY, HOSTS),
    ])?;
    let paired = conventional_aws_pair(&set, &no_declared())
        .pair
        .ok_or("expected a pair")?;
    assert!(paired.session_token().is_none());
    Ok(())
}

#[test]
fn a_secret_without_an_access_key_id_is_reported() -> TestResult {
    let set = trio(&[("secret", CONVENTIONAL_AWS_SECRET_ACCESS_KEY, HOSTS)])?;
    let result = conventional_aws_pair(&set, &no_declared());
    assert!(result.pair.is_none());
    assert_eq!(
        result.unpaired,
        vec![(name("secret")?, UnpairedReason::SecretAccessKeyWithoutAccessKeyId)]
    );
    Ok(())
}

#[test]
fn an_access_key_id_alone_does_not_pair() -> TestResult {
    let set = trio(&[("id", CONVENTIONAL_AWS_ACCESS_KEY_ID, HOSTS)])?;
    let result = conventional_aws_pair(&set, &no_declared());
    assert!(result.pair.is_none() && result.unpaired.is_empty());
    Ok(())
}

#[test]
fn nothing_conventional_yields_nothing() -> TestResult {
    let set = trio(&[("other", "OTHER", HOSTS)])?;
    let result = conventional_aws_pair(&set, &no_declared());
    assert!(result.pair.is_none() && result.unpaired.is_empty());
    Ok(())
}

#[test]
fn an_extracted_or_denied_member_does_not_pair() -> TestResult {
    let extract = Extract::Regex(ExtractPattern::new("(a+)")?);
    let extracted = binding(
        "id",
        CONVENTIONAL_AWS_ACCESS_KEY_ID,
        SecretMode::Mask,
        Some(extract),
        HOSTS,
    )?;
    let secret = mask_binding("secret", CONVENTIONAL_AWS_SECRET_ACCESS_KEY, HOSTS)?;
    let result = conventional_aws_pair(&bindings(vec![extracted, secret])?, &no_declared());
    assert!(result.pair.is_none());
    assert_eq!(result.unpaired.len(), 1);

    let denied = binding("id", CONVENTIONAL_AWS_ACCESS_KEY_ID, SecretMode::Deny, None, HOSTS)?;
    let secret = mask_binding("secret", CONVENTIONAL_AWS_SECRET_ACCESS_KEY, HOSTS)?;
    let result = conventional_aws_pair(&bindings(vec![denied, secret])?, &no_declared());
    assert!(result.pair.is_none());
    Ok(())
}

#[test]
fn a_declared_pair_claiming_a_member_turns_it_off() -> TestResult {
    let set = trio(&[
        ("id", CONVENTIONAL_AWS_ACCESS_KEY_ID, HOSTS),
        ("secret", CONVENTIONAL_AWS_SECRET_ACCESS_KEY, HOSTS),
        ("other-id", "OTHER_ID", HOSTS),
        ("other-secret", "OTHER_SECRET", HOSTS),
    ])?;
    let declared = AwsCredentialPairs::new(vec![pair("other-id", "id", None)?], &set)?;
    let result = conventional_aws_pair(&set, &declared);
    assert!(result.pair.is_none() && result.unpaired.is_empty());
    Ok(())
}

#[test]
fn a_member_at_other_hosts_is_left_out() -> TestResult {
    let set = trio(&[
        ("id", CONVENTIONAL_AWS_ACCESS_KEY_ID, HOSTS),
        ("secret", CONVENTIONAL_AWS_SECRET_ACCESS_KEY, &["elsewhere.example.com"]),
    ])?;
    let result = conventional_aws_pair(&set, &no_declared());
    assert!(result.pair.is_none());
    assert_eq!(
        result.unpaired,
        vec![(name("secret")?, UnpairedReason::HostsDifferFromAccessKeyId)]
    );

    let set = trio(&[
        ("id", CONVENTIONAL_AWS_ACCESS_KEY_ID, HOSTS),
        ("secret", CONVENTIONAL_AWS_SECRET_ACCESS_KEY, HOSTS),
        ("token", CONVENTIONAL_AWS_SESSION_TOKEN, &["elsewhere.example.com"]),
    ])?;
    let result = conventional_aws_pair(&set, &no_declared());
    let paired = result.pair.ok_or("expected a pair")?;
    assert!(paired.session_token().is_none());
    assert_eq!(
        result.unpaired,
        vec![(name("token")?, UnpairedReason::HostsDifferFromAccessKeyId)]
    );
    Ok(())
}
