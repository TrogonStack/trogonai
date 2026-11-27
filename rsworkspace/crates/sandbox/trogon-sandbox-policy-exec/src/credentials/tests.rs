use super::*;
use trogon_sandbox_contract::{
    ClaimName, DomainPattern, EnvVarName, ExtractPattern, InjectHosts, JwtMasking, LiteralPath, ResponseHeaderStrip,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A process environment with nothing set.
struct EmptyEnv;

impl ReadEnv for EmptyEnv {
    fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotPresent)
    }
}

fn binding(
    name: &str,
    target: SecretTarget,
    mode: SecretMode,
    extract: Option<Extract>,
    on_extract_no_match: ExtractNoMatch,
) -> Result<SecretBinding, Box<dyn std::error::Error>> {
    let name = SecretName::new(name)?;
    Ok(SecretBinding::new(SecretBindingParts {
        source: host_source_request(&name),
        name,
        target,
        mode,
        extract,
        mask_duplicates: MaskDuplicates::Disabled,
        on_extract_no_match,
        strip_response_headers: ResponseHeaderStrip::default(),
        inject_hosts: InjectHosts::new(vec![DomainPattern::new("api.example.com")?])?,
        inject_scope: Default::default(),
    })?)
}

fn env_binding(
    name: &str,
    var: &str,
    mode: SecretMode,
    extract: Option<Extract>,
) -> Result<SecretBinding, Box<dyn std::error::Error>> {
    binding(
        name,
        SecretTarget::Env(EnvVarName::new(var)?),
        mode,
        extract,
        ExtractNoMatch::default(),
    )
}

fn file_binding(
    name: &str,
    path: &std::path::Path,
    extract: Option<Extract>,
    on_extract_no_match: ExtractNoMatch,
) -> Result<SecretBinding, Box<dyn std::error::Error>> {
    binding(
        name,
        SecretTarget::File(LiteralPath::new(path.to_str().ok_or("utf8 path")?)?),
        SecretMode::Mask,
        extract,
        on_extract_no_match,
    )
}

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn a_command_variable_wins_and_an_unset_one_leaves_the_binding_out() -> TestResult {
    let bindings = [
        env_binding("token", "SANDBOX_CRED_TEST_TOKEN", SecretMode::Mask, None)?,
        env_binding("other", "SANDBOX_CRED_TEST_UNSET_VARIABLE", SecretMode::Mask, None)?,
    ];
    let command = env(&[("SANDBOX_CRED_TEST_TOKEN", "from-command")]);
    let config = env(&[("SANDBOX_CRED_TEST_TOKEN", "from-config")]);
    let resolved = resolve(&bindings, &[&command, &config], &EmptyEnv)?;
    assert_eq!(resolved.bindings.len(), 1);
    assert_eq!(resolved.values.get("token").map(String::as_str), Some("from-command"));
    assert!(resolved.withheld_env.contains("SANDBOX_CRED_TEST_TOKEN"));
    assert!(resolved.withheld_env.contains("SANDBOX_CRED_TEST_UNSET_VARIABLE"));
    Ok(())
}

#[test]
fn a_value_that_is_not_a_jwt_passes_through_where_decode_jwt_cannot_mask_it() -> TestResult {
    let binding = env_binding(
        "token",
        "SANDBOX_CRED_TEST_NOT_JWT",
        SecretMode::Mask,
        Some(Extract::Jwt(JwtMasking::default())),
    )?;
    let command = env(&[("SANDBOX_CRED_TEST_NOT_JWT", "plain-secret")]);
    let resolved = resolve(&[binding], &[&command], &EmptyEnv)?;
    assert!(resolved.bindings.is_empty());
    assert!(resolved.values.is_empty());
    assert!(!resolved.withheld_env.contains("SANDBOX_CRED_TEST_NOT_JWT"));
    Ok(())
}

#[test]
fn a_jwt_none_of_whose_named_claims_is_a_string_passes_through() -> TestResult {
    let masking = JwtMasking {
        mask_claims: vec![ClaimName::new("absent")?],
        pattern: None,
    };
    let binding = env_binding(
        "token",
        "SANDBOX_CRED_TEST_NO_CLAIM",
        SecretMode::Mask,
        Some(Extract::Jwt(masking)),
    )?;
    let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhIn0.c2ln";
    let command = env(&[("SANDBOX_CRED_TEST_NO_CLAIM", jwt)]);
    let resolved = resolve(&[binding], &[&command], &EmptyEnv)?;
    assert!(resolved.bindings.is_empty());
    assert!(!resolved.withheld_env.contains("SANDBOX_CRED_TEST_NO_CLAIM"));
    Ok(())
}

#[test]
fn a_verified_jwt_is_masked() -> TestResult {
    let binding = env_binding(
        "token",
        "SANDBOX_CRED_TEST_JWT",
        SecretMode::Mask,
        Some(Extract::Jwt(JwtMasking::default())),
    )?;
    let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhIn0.c2ln";
    let command = env(&[("SANDBOX_CRED_TEST_JWT", jwt)]);
    let resolved = resolve(&[binding], &[&command], &EmptyEnv)?;
    assert_eq!(resolved.bindings.len(), 1);
    assert!(resolved.withheld_env.contains("SANDBOX_CRED_TEST_JWT"));
    Ok(())
}

#[test]
fn a_whole_file_is_registered_verbatim() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("token");
    std::fs::write(&path, "file-secret\n")?;
    let binding = file_binding("file", &path, None, ExtractNoMatch::default())?;
    let resolved = resolve(&[binding], &[], &EmptyEnv)?;
    assert_eq!(resolved.values.get("file").map(String::as_str), Some("file-secret\n"));
    Ok(())
}

#[test]
fn an_extract_rule_takes_its_first_capture() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config");
    std::fs::write(&path, "user=a\ntoken=abc123\n")?;
    let rule = Extract::Regex(ExtractPattern::new("token=(\\w+)")?);
    let binding = file_binding("file", &path, Some(rule), ExtractNoMatch::default())?;
    let resolved = resolve(&[binding], &[], &EmptyEnv)?;
    assert_eq!(resolved.values.get("file").map(String::as_str), Some("abc123"));
    Ok(())
}

#[test]
fn a_file_that_does_not_exist_leaves_the_binding_out() -> TestResult {
    let dir = tempfile::tempdir()?;
    let binding = file_binding("file", &dir.path().join("missing"), None, ExtractNoMatch::default())?;
    let resolved = resolve(&[binding], &[], &EmptyEnv)?;
    assert!(resolved.bindings.is_empty());
    Ok(())
}

#[test]
fn a_file_that_cannot_be_read_as_text_leaves_the_binding_out() -> TestResult {
    let dir = tempfile::tempdir()?;
    let binary = dir.path().join("binary");
    std::fs::write(&binary, [0xff, 0xfe, 0x00, 0x80])?;
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder)?;
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut paths = vec![binary.clone(), folder];
    #[cfg(unix)]
    {
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&binary, &link)?;
        paths.push(link);
    }
    for path in paths {
        let binding = file_binding("file", &path, None, ExtractNoMatch::default())?;
        let resolved = resolve(&[binding], &[], &EmptyEnv)?;
        assert!(resolved.bindings.is_empty(), "{}", path.display());
        assert!(resolved.values.is_empty());
    }
    Ok(())
}

#[test]
fn an_extract_rule_that_finds_nothing_follows_its_policy() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config");
    std::fs::write(&path, "nothing here\n")?;
    let rule =
        || -> Result<Extract, Box<dyn std::error::Error>> { Ok(Extract::Regex(ExtractPattern::new("token=(\\w+)")?)) };

    let erroring = file_binding("file", &path, Some(rule()?), ExtractNoMatch::Error)?;
    assert!(resolve(&[erroring], &[], &EmptyEnv).is_err());

    let warning = file_binding("file", &path, Some(rule()?), ExtractNoMatch::Warn)?;
    assert!(resolve(&[warning], &[], &EmptyEnv)?.bindings.is_empty());

    let denying = file_binding("file", &path, Some(rule()?), ExtractNoMatch::Deny)?;
    let resolved = resolve(&[denying], &[], &EmptyEnv)?;
    assert_eq!(
        resolved.bindings.first().map(SecretBinding::mode),
        Some(SecretMode::Deny)
    );
    assert!(resolved.values.is_empty());
    Ok(())
}

#[test]
fn a_denied_variable_is_withheld() -> TestResult {
    let binding = env_binding("deny", "SANDBOX_CRED_TEST_DENIED", SecretMode::Deny, None)?;
    let resolved = resolve(&[binding], &[], &EmptyEnv)?;
    assert!(resolved.withheld_env.contains("SANDBOX_CRED_TEST_DENIED"));
    assert!(resolved.values.is_empty());
    Ok(())
}
