use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    AwsCredentialPairBinding, AwsCredentialPairBindingError, AwsCredentialPairs, AwsCredentialPairsError,
    CredentialProvider, CredentialRequest, CredentialScope, DomainPattern, Extract, ExtractNoMatch, ExtractPattern,
    InjectHosts, InjectHostsError, InjectMethod, InjectMethodSet, InjectMethodSetError, InjectPathPrefix,
    InjectPathPrefixError, InjectScope, InjectScopeVersion, InjectScopeVersionError, MaskDuplicates, PullSecretId,
    ResponseHeaderStrip, SecretBinding, SecretBindingError, SecretBindingParts, SecretBindings, SecretBindingsError,
    SecretMode, SecretName, SecretTarget, StrippedHeader, StrippedHeaderError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn rejects<T: serde::de::DeserializeOwned>(value: Value) -> TestResult {
    assert_eq!(error_of(serde_json::from_value::<T>(value.clone()))?.classify(), Data);
    assert_eq!(
        error_of(serde_json::from_str::<T>(&value.to_string()))?.classify(),
        Data
    );
    Ok(())
}

fn source() -> CredentialRequest {
    CredentialRequest {
        provider: CredentialProvider::Registry,
        pull_secret_id: PullSecretId::new("registry-pull"),
        scope: CredentialScope::RegistryPull {
            repository_glob: "*".to_string(),
        },
        ttl_seconds: 60,
        requester: None,
    }
}

fn name(value: &str) -> Result<SecretName, Box<dyn std::error::Error>> {
    Ok(SecretName::new(value)?)
}

fn hosts(values: &[&str]) -> Result<InjectHosts, Box<dyn std::error::Error>> {
    let patterns = values
        .iter()
        .map(|value| DomainPattern::new(value))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(InjectHosts::new(patterns)?)
}

fn parts(secret: &str) -> Result<SecretBindingParts, Box<dyn std::error::Error>> {
    Ok(SecretBindingParts {
        name: name(secret)?,
        target: SecretTarget::Env(secret.parse()?),
        mode: SecretMode::Mask,
        extract: None,
        mask_duplicates: MaskDuplicates::Disabled,
        on_extract_no_match: ExtractNoMatch::Error,
        strip_response_headers: ResponseHeaderStrip::default(),
        inject_hosts: hosts(&["api.example.com"])?,
        inject_scope: InjectScope::default(),
        source: source(),
    })
}

fn minimal_json(secret: &str) -> Value {
    json!({
        "name": secret,
        "target": { "env": secret },
        "mode": "mask",
        "inject_hosts": ["api.example.com"],
        "source": serde_json::to_value(source()).unwrap_or(Value::Null),
    })
}

#[test]
fn a_minimal_binding_omits_every_defaulted_field() -> TestResult {
    let binding = SecretBinding::new(parts("API_KEY")?)?;
    let wire = serde_json::to_value(&binding)?;
    assert_eq!(wire, minimal_json("API_KEY"));
    assert_eq!(serde_json::from_value::<SecretBinding>(wire)?, binding);
    assert_eq!(binding.name().as_str(), "API_KEY");
    assert_eq!(binding.target().to_string(), "env 'API_KEY'");
    assert_eq!(binding.mode(), SecretMode::Mask);
    assert_eq!(binding.extract(), None);
    assert_eq!(binding.mask_duplicates(), MaskDuplicates::Disabled);
    assert_eq!(binding.on_extract_no_match(), ExtractNoMatch::Error);
    assert!(binding.strip_response_headers().is_empty());
    assert_eq!(binding.inject_hosts().hosts().len(), 1);
    assert!(binding.inject_scope().is_default());
    assert_eq!(binding.source(), &source());
    assert!(binding.is_whole_value_mask());
    Ok(())
}

#[test]
fn a_full_binding_round_trips_with_flat_scope_fields() -> TestResult {
    let mut full = parts("NETRC")?;
    full.target = SecretTarget::File("/home/u/./.netrc".parse()?);
    full.extract = Some(Extract::Regex(ExtractPattern::new(r"password (\S+)")?));
    full.mask_duplicates = MaskDuplicates::Enabled;
    full.on_extract_no_match = ExtractNoMatch::Warn;
    full.strip_response_headers = ResponseHeaderStrip::new([
        StrippedHeader::new("Set_Cookie")?,
        StrippedHeader::new("x.token")?,
        StrippedHeader::new("set-cookie")?,
    ]);
    full.inject_scope = InjectScope {
        path_prefix: Some(InjectPathPrefix::new("/v1")?),
        methods: Some(InjectMethodSet::new(vec![InjectMethod::Get, InjectMethod::Post])?),
        scope_version: InjectScopeVersion::V2,
    };
    let binding = SecretBinding::new(full)?;
    let wire = serde_json::to_value(&binding)?;
    assert_eq!(
        wire,
        json!({
            "name": "NETRC",
            "target": { "file": "/home/u/.netrc" },
            "mode": "mask",
            "extract": { "regex": "password (\\S+)" },
            "mask_duplicates": true,
            "on_extract_no_match": "warn",
            "strip_response_headers": ["set-cookie", "x-token"],
            "inject_hosts": ["api.example.com"],
            "path_prefix": "/v1",
            "methods": ["GET", "POST"],
            "scope_version": 2,
            "source": serde_json::to_value(source())?,
        })
    );
    assert_eq!(serde_json::from_value::<SecretBinding>(wire.clone())?, binding);
    assert_eq!(serde_json::from_str::<SecretBinding>(&wire.to_string())?, binding);
    assert!(!binding.is_whole_value_mask());
    assert_eq!(binding.extract().map(Extract::fits_env_target), Some(true));
    Ok(())
}

#[test]
fn wire_fields_are_checked_on_deserialization() -> TestResult {
    let with = |key: &str, value: Value| {
        let mut wire = minimal_json("API_KEY");
        wire[key] = value;
        wire
    };
    rejects::<SecretBinding>(with("inject_hosts", json!([])))?;
    rejects::<SecretBinding>(with("mode", json!("allow")))?;
    rejects::<SecretBinding>(with("name", json!("bad name")))?;
    rejects::<SecretBinding>(with("target", json!({ "file": "relative" })))?;
    rejects::<SecretBinding>(with("on_extract_no_match", json!("ignore")))?;
    rejects::<SecretBinding>(with("strip_response_headers", json!(["bad header"])))?;
    rejects::<SecretBinding>(with("path_prefix", json!("v1")))?;
    rejects::<SecretBinding>(with("methods", json!([])))?;
    rejects::<SecretBinding>(with("scope_version", json!(3)))?;
    rejects::<SecretBinding>(with("mask_duplicates", json!("yes")))?;
    let mut missing = minimal_json("API_KEY");
    if let Some(object) = missing.as_object_mut() {
        object.remove("inject_hosts");
    }
    rejects::<SecretBinding>(missing)?;
    let wire = with("extract", json!({ "regex": "no capture" }));
    rejects::<SecretBinding>(wire)?;
    rejects::<SecretBinding>(with("on_extract_no_match", json!("deny")))?;
    Ok(())
}

#[test]
fn each_binding_rule_has_its_own_error() -> TestResult {
    let regex = Extract::Regex(ExtractPattern::new("(a)")?);

    let mut deny_extract = parts("A")?;
    deny_extract.mode = SecretMode::Deny;
    deny_extract.extract = Some(regex.clone());
    let error = error_of(SecretBinding::new(deny_extract))?;
    assert_eq!(error, SecretBindingError::ExtractOnDeny(name("A")?));
    assert_eq!(
        error.to_string(),
        "secret binding 'A' has mode deny, which does not take an extract pattern"
    );

    let mut env_line = parts("B")?;
    env_line.extract = Some(Extract::EnvLine("B".parse()?));
    let error = error_of(SecretBinding::new(env_line))?;
    assert_eq!(error, SecretBindingError::ExtractOnEnv(name("B")?));
    assert_eq!(
        error.to_string(),
        "secret binding 'B' targets an environment variable, which takes only a regex or jwt extract rule"
    );

    let mut policy = parts("C")?;
    policy.on_extract_no_match = ExtractNoMatch::Deny;
    let error = error_of(SecretBinding::new(policy))?;
    assert_eq!(error, SecretBindingError::NoMatchPolicyWithoutExtract(name("C")?));
    assert_eq!(
        error.to_string(),
        "secret binding 'C' sets on_extract_no_match without an extract rule"
    );

    let mut strip = parts("D")?;
    strip.mode = SecretMode::Deny;
    strip.strip_response_headers = ResponseHeaderStrip::new([StrippedHeader::new("set-cookie")?]);
    let error = error_of(SecretBinding::new(strip))?;
    assert_eq!(error, SecretBindingError::StripOnDeny(name("D")?));
    assert_eq!(
        error.to_string(),
        "secret binding 'D' has mode deny, which does not strip response headers"
    );

    for scope in [
        InjectScope {
            path_prefix: Some(InjectPathPrefix::new("/v1")?),
            ..InjectScope::default()
        },
        InjectScope {
            methods: Some(InjectMethodSet::new(vec![InjectMethod::Get])?),
            ..InjectScope::default()
        },
    ] {
        let mut deny_scope = parts("E")?;
        deny_scope.mode = SecretMode::Deny;
        deny_scope.inject_scope = scope;
        let error = error_of(SecretBinding::new(deny_scope))?;
        assert_eq!(error, SecretBindingError::ScopeOnDeny(name("E")?));
        assert_eq!(
            error.to_string(),
            "secret binding 'E' has mode deny, which takes no path_prefix or methods"
        );
    }

    let mut deny_version = parts("F")?;
    deny_version.mode = SecretMode::Deny;
    deny_version.inject_scope.scope_version = InjectScopeVersion::V2;
    let denied = SecretBinding::new(deny_version)?;
    assert!(!denied.is_whole_value_mask());
    assert_eq!(serde_json::to_value(&denied)?["mode"], json!("deny"));

    let mut file_regex = parts("G")?;
    file_regex.target = SecretTarget::File("/etc/g".parse()?);
    file_regex.extract = Some(Extract::EnvLine("G".parse()?));
    assert!(SecretBinding::new(file_regex).is_ok());

    let wire = {
        let mut wire = minimal_json("A");
        wire["mode"] = json!("deny");
        wire["extract"] = json!({ "regex": "(a)" });
        wire
    };
    rejects::<SecretBinding>(wire)?;
    Ok(())
}

#[test]
fn bindings_enforce_the_collection_rules() -> TestResult {
    let binding =
        |secret: &str| -> Result<SecretBinding, Box<dyn std::error::Error>> { Ok(SecretBinding::new(parts(secret)?)?) };
    assert!(SecretBindings::default().is_empty());
    assert!(SecretBindings::new(Vec::new())?.is_empty());

    let two = SecretBindings::new(vec![binding("A")?, binding("B")?])?;
    assert_eq!(two.len(), 2);
    assert!(!two.is_empty());
    assert_eq!(two.bindings()[1].name().as_str(), "B");
    let wire = serde_json::to_value(&two)?;
    assert_eq!(wire, json!([minimal_json("A"), minimal_json("B")]));
    assert_eq!(serde_json::from_value::<SecretBindings>(wire)?, two);
    assert_eq!(Vec::from(two.clone()).len(), 2);
    assert_eq!(SecretBindings::try_from(Vec::from(two.clone()))?, two);

    let mut many = Vec::new();
    for index in 0..=SecretBindings::MAX {
        many.push(binding(&format!("S{index}"))?);
    }
    let error = error_of(SecretBindings::new(many.clone()))?;
    assert_eq!(error, SecretBindingsError::TooMany { max: 64, got: 65 });
    assert_eq!(error.to_string(), "secret bindings support at most 64 entries, got 65");
    many.pop();
    assert_eq!(SecretBindings::new(many)?.len(), 64);

    let error = error_of(SecretBindings::new(vec![binding("A")?, binding("A")?]))?;
    assert_eq!(error, SecretBindingsError::DuplicateName(name("A")?));
    assert_eq!(error.to_string(), "duplicate secret binding name 'A'");

    let mut same_target = parts("B")?;
    same_target.target = SecretTarget::Env("A".parse()?);
    let error = error_of(SecretBindings::new(vec![
        binding("A")?,
        SecretBinding::new(same_target)?,
    ]))?;
    assert_eq!(
        error,
        SecretBindingsError::DuplicateTarget(SecretTarget::Env("A".parse()?))
    );
    assert_eq!(error.to_string(), "duplicate secret binding target env 'A'");

    rejects::<SecretBindings>(json!([minimal_json("A"), minimal_json("A")]))?;
    rejects::<SecretBindings>(json!({}))?;
    Ok(())
}

#[test]
fn targets_modes_and_flags_have_their_wire_forms() -> TestResult {
    let file: SecretTarget = serde_json::from_value(json!({ "file": "/home/u/./.netrc" }))?;
    assert_eq!(serde_json::to_value(&file)?, json!({ "file": "/home/u/.netrc" }));
    assert_eq!(file.to_string(), "file '/home/u/.netrc'");
    rejects::<SecretTarget>(json!({ "file": "relative" }))?;
    rejects::<SecretTarget>(json!({ "env": "1BAD" }))?;
    rejects::<SecretTarget>(json!({ "socket": "x" }))?;

    for (mode, text) in [(SecretMode::Mask, "mask"), (SecretMode::Deny, "deny")] {
        assert_eq!(serde_json::to_value(mode)?, json!(text));
        assert_eq!(serde_json::from_value::<SecretMode>(json!(text))?, mode);
    }
    rejects::<SecretMode>(json!("allow"))?;

    for (policy, text) in [
        (ExtractNoMatch::Error, "error"),
        (ExtractNoMatch::Warn, "warn"),
        (ExtractNoMatch::Deny, "deny"),
    ] {
        assert_eq!(serde_json::to_value(policy)?, json!(text));
        assert_eq!(serde_json::from_value::<ExtractNoMatch>(json!(text))?, policy);
        assert_eq!(policy.is_default(), policy == ExtractNoMatch::Error);
    }
    rejects::<ExtractNoMatch>(json!("ignore"))?;

    assert_eq!(serde_json::to_value(MaskDuplicates::Enabled)?, json!(true));
    assert_eq!(serde_json::to_value(MaskDuplicates::Disabled)?, json!(false));
    assert_eq!(
        serde_json::from_value::<MaskDuplicates>(json!(true))?,
        MaskDuplicates::Enabled
    );
    assert_eq!(
        serde_json::from_value::<MaskDuplicates>(json!(false))?,
        MaskDuplicates::Disabled
    );
    assert!(MaskDuplicates::Enabled.is_enabled());
    assert!(!MaskDuplicates::Enabled.is_default());
    assert!(MaskDuplicates::Disabled.is_default());
    assert_eq!(MaskDuplicates::from(true), MaskDuplicates::Enabled);
    assert!(bool::from(MaskDuplicates::Enabled));
    rejects::<MaskDuplicates>(json!("true"))?;
    Ok(())
}

#[test]
fn path_prefixes_follow_the_grammar() -> TestResult {
    for accepted in ["/", "/v1", "/a/b-c_d.e~f", "/a:b@c", "/!$&'()*+,;="] {
        let prefix = InjectPathPrefix::new(accepted)?;
        assert_eq!(prefix.as_str(), accepted);
        assert_eq!(prefix.to_string(), accepted);
        assert_eq!(accepted.parse::<InjectPathPrefix>()?, prefix);
        assert_eq!(InjectPathPrefix::try_from(accepted.to_owned())?, prefix);
        assert_eq!(serde_json::to_value(&prefix)?, json!(accepted));
        assert_eq!(serde_json::from_value::<InjectPathPrefix>(json!(accepted))?, prefix);
        assert_eq!(String::from(prefix), accepted);
    }
    let long = format!("/{}", "a".repeat(InjectPathPrefix::MAX_LEN));
    let cases = [
        (
            "v1",
            InjectPathPrefixError::NotAbsolute,
            "path_prefix must start with '/'",
        ),
        (
            "",
            InjectPathPrefixError::NotAbsolute,
            "path_prefix must start with '/'",
        ),
        (
            long.as_str(),
            InjectPathPrefixError::TooLong { max: 1024 },
            "path_prefix must be at most 1024 bytes",
        ),
        (
            "/a%2fb",
            InjectPathPrefixError::Character,
            "path_prefix may contain only unreserved characters, sub-delimiters, ':', '@' and '/' (no '%', '?', '#', spaces or control characters)",
        ),
        ("/a b", InjectPathPrefixError::Character, ""),
        ("/a?b", InjectPathPrefixError::Character, ""),
        (
            "/a/",
            InjectPathPrefixError::TrailingSlash,
            "path_prefix must not end with '/' unless it is '/'",
        ),
        (
            "//a",
            InjectPathPrefixError::Segment,
            "path_prefix must not contain empty, '.' or '..' segments",
        ),
        ("/a/./b", InjectPathPrefixError::Segment, ""),
        ("/a/../b", InjectPathPrefixError::Segment, ""),
    ];
    for (input, expected, message) in cases {
        let error = error_of(InjectPathPrefix::new(input))?;
        assert_eq!(error, expected);
        if !message.is_empty() {
            assert_eq!(error.to_string(), message);
        }
        assert_eq!(error_of(input.parse::<InjectPathPrefix>())?, expected);
        rejects::<InjectPathPrefix>(json!(input))?;
    }
    Ok(())
}

#[test]
fn methods_form_a_non_empty_set_without_repeats() -> TestResult {
    let all = [
        (InjectMethod::Get, "GET"),
        (InjectMethod::Head, "HEAD"),
        (InjectMethod::Post, "POST"),
        (InjectMethod::Put, "PUT"),
        (InjectMethod::Patch, "PATCH"),
        (InjectMethod::Delete, "DELETE"),
        (InjectMethod::Options, "OPTIONS"),
    ];
    for (method, text) in all {
        assert_eq!(method.as_str(), text);
        assert_eq!(method.to_string(), text);
        assert_eq!(serde_json::to_value(method)?, json!(text));
        assert_eq!(serde_json::from_value::<InjectMethod>(json!(text))?, method);
    }
    rejects::<InjectMethod>(json!("get"))?;
    rejects::<InjectMethod>(json!("TRACE"))?;

    let set = InjectMethodSet::new(vec![InjectMethod::Put, InjectMethod::Get])?;
    assert_eq!(set.methods(), [InjectMethod::Put, InjectMethod::Get]);
    assert!(set.contains(InjectMethod::Get));
    assert!(!set.contains(InjectMethod::Delete));
    assert_eq!(serde_json::to_value(&set)?, json!(["PUT", "GET"]));
    assert_eq!(serde_json::from_value::<InjectMethodSet>(json!(["PUT", "GET"]))?, set);
    assert_eq!(Vec::from(set.clone()), vec![InjectMethod::Put, InjectMethod::Get]);
    assert_eq!(InjectMethodSet::try_from(Vec::from(set.clone()))?, set);

    let error = error_of(InjectMethodSet::new(Vec::new()))?;
    assert_eq!(error, InjectMethodSetError::Empty);
    assert_eq!(error.to_string(), "methods must not be empty");
    let error = error_of(InjectMethodSet::new(vec![InjectMethod::Get, InjectMethod::Get]))?;
    assert_eq!(error, InjectMethodSetError::Duplicate(InjectMethod::Get));
    assert_eq!(error.to_string(), "methods must not repeat 'GET'");
    rejects::<InjectMethodSet>(json!([]))?;
    rejects::<InjectMethodSet>(json!(["GET", "GET"]))?;
    Ok(())
}

#[test]
fn scope_versions_and_scopes_have_defaults() -> TestResult {
    assert_eq!(InjectScopeVersion::default(), InjectScopeVersion::V1);
    assert!(InjectScopeVersion::V1.is_default());
    assert!(!InjectScopeVersion::V2.is_default());
    assert_eq!(serde_json::to_value(InjectScopeVersion::V1)?, json!(1));
    assert_eq!(
        serde_json::from_value::<InjectScopeVersion>(json!(2))?,
        InjectScopeVersion::V2
    );
    assert_eq!(u8::from(InjectScopeVersion::V2), 2);
    assert_eq!(InjectScopeVersion::try_from(1)?, InjectScopeVersion::V1);
    let error = error_of(InjectScopeVersion::try_from(3))?;
    assert_eq!(error, InjectScopeVersionError::Unknown(3));
    assert_eq!(error.to_string(), "unsupported scope_version 3, expected 1 or 2");
    rejects::<InjectScopeVersion>(json!(0))?;
    rejects::<InjectScopeVersion>(json!(300))?;

    let scope: InjectScope = serde_json::from_value(json!({}))?;
    assert!(scope.is_default());
    assert!(scope.is_request_independent());
    assert_eq!(serde_json::to_value(&scope)?, json!({}));
    let scope = InjectScope {
        scope_version: InjectScopeVersion::V2,
        ..InjectScope::default()
    };
    assert!(!scope.is_default());
    assert!(scope.is_request_independent());
    assert_eq!(serde_json::to_value(&scope)?, json!({ "scope_version": 2 }));
    Ok(())
}

#[test]
fn inject_hosts_reject_an_empty_list() -> TestResult {
    let error = error_of(InjectHosts::new(Vec::new()))?;
    assert_eq!(error, InjectHostsError::Empty);
    assert_eq!(error.to_string(), "inject_hosts must contain at least one host");
    let hosts = hosts(&["a.example.com", "*.example.org"])?;
    assert_eq!(hosts.hosts().len(), 2);
    let wire = json!(["a.example.com", "*.example.org"]);
    assert_eq!(serde_json::to_value(&hosts)?, wire);
    assert_eq!(serde_json::from_value::<InjectHosts>(wire)?, hosts);
    assert_eq!(InjectHosts::try_from(Vec::from(hosts.clone()))?, hosts);
    rejects::<InjectHosts>(json!([]))?;
    rejects::<InjectHosts>(json!("a.example.com"))?;
    Ok(())
}

#[test]
fn stripped_headers_fold_and_deduplicate() -> TestResult {
    let header = StrippedHeader::new("Set_Cookie.X")?;
    assert_eq!(header.as_str(), "set-cookie-x");
    assert_eq!(header.to_string(), "set-cookie-x");
    assert_eq!("Set-Cookie-X".parse::<StrippedHeader>()?, header);
    assert_eq!(StrippedHeader::try_from("set_cookie_x".to_owned())?, header);
    assert_eq!(serde_json::to_value(&header)?, json!("set-cookie-x"));
    assert_eq!(serde_json::from_value::<StrippedHeader>(json!("SET.COOKIE.X"))?, header);
    assert_eq!(String::from(header.clone()), "set-cookie-x");

    for invalid in ["", "bad header", "caf\u{e9}", "a:b"] {
        let error = error_of(StrippedHeader::new(invalid))?;
        assert_eq!(error, StrippedHeaderError::Invalid);
        assert_eq!(
            error.to_string(),
            "response header name must be a non-empty token of letters, digits, '-', '_' and '.'"
        );
        assert_eq!(error_of(invalid.parse::<StrippedHeader>())?, error);
        rejects::<StrippedHeader>(json!(invalid))?;
    }

    let strip: ResponseHeaderStrip = serde_json::from_value(json!(["A", "a", "B_c"]))?;
    assert_eq!(strip.headers().len(), 2);
    assert_eq!(serde_json::to_value(&strip)?, json!(["a", "b-c"]));
    assert!(!strip.is_empty());
    assert!(ResponseHeaderStrip::default().is_empty());
    assert_eq!(Vec::from(strip.clone()).len(), 2);
    assert_eq!(ResponseHeaderStrip::from(Vec::from(strip.clone())), strip);
    rejects::<ResponseHeaderStrip>(json!(["bad header"]))?;
    Ok(())
}

fn aws_binding(secret: &str, host: &str) -> Result<SecretBinding, Box<dyn std::error::Error>> {
    let mut parts = parts(secret)?;
    parts.inject_hosts = hosts(&[host])?;
    Ok(SecretBinding::new(parts)?)
}

#[test]
fn aws_pairs_hold_distinct_members() -> TestResult {
    let pair = AwsCredentialPairBinding::new(name("AKID")?, name("SECRET")?, None)?;
    assert_eq!(pair.access_key_id().as_str(), "AKID");
    assert_eq!(pair.secret_access_key().as_str(), "SECRET");
    assert_eq!(pair.session_token(), None);
    assert_eq!(pair.members().count(), 2);
    let wire = json!({ "access_key_id": "AKID", "secret_access_key": "SECRET" });
    assert_eq!(serde_json::to_value(&pair)?, wire);
    assert_eq!(serde_json::from_value::<AwsCredentialPairBinding>(wire)?, pair);

    let pair = AwsCredentialPairBinding::new(name("AKID")?, name("SECRET")?, Some(name("TOKEN")?))?;
    assert_eq!(pair.session_token().map(SecretName::as_str), Some("TOKEN"));
    assert_eq!(
        pair.members().map(SecretName::as_str).collect::<Vec<_>>(),
        ["AKID", "SECRET", "TOKEN"]
    );
    let wire = json!({ "access_key_id": "AKID", "secret_access_key": "SECRET", "session_token": "TOKEN" });
    assert_eq!(serde_json::to_value(&pair)?, wire);
    assert_eq!(
        serde_json::from_str::<AwsCredentialPairBinding>(&wire.to_string())?,
        pair
    );

    for (first, second, third, repeated) in [
        ("A", "A", None, "A"),
        ("A", "B", Some("A"), "A"),
        ("A", "B", Some("B"), "B"),
    ] {
        let error = error_of(AwsCredentialPairBinding::new(
            name(first)?,
            name(second)?,
            third.map(name).transpose()?,
        ))?;
        assert_eq!(error, AwsCredentialPairBindingError::DuplicateMember(name(repeated)?));
        assert_eq!(
            error.to_string(),
            format!("AWS credential pair member '{repeated}' appears more than once")
        );
    }
    rejects::<AwsCredentialPairBinding>(json!({ "access_key_id": "A", "secret_access_key": "A" }))?;
    rejects::<AwsCredentialPairBinding>(json!({ "access_key_id": "A" }))?;
    Ok(())
}

#[test]
fn aws_pairs_are_checked_against_their_bindings() -> TestResult {
    let bindings = SecretBindings::new(vec![
        aws_binding("AKID", "a.example.com")?,
        aws_binding("SECRET", "a.example.com")?,
        aws_binding("TOKEN", "a.example.com")?,
        aws_binding("OTHER", "b.example.com")?,
    ])?;
    let pair = |access: &str,
                secret: &str,
                token: Option<&str>|
     -> Result<AwsCredentialPairBinding, Box<dyn std::error::Error>> {
        Ok(AwsCredentialPairBinding::new(
            name(access)?,
            name(secret)?,
            token.map(name).transpose()?,
        )?)
    };

    let none = AwsCredentialPairs::new(Vec::new(), &bindings)?;
    assert!(none.pairs().is_empty());
    assert_eq!(AwsCredentialPairs::default(), none);
    let ok = AwsCredentialPairs::new(vec![pair("AKID", "SECRET", Some("TOKEN"))?], &bindings)?;
    assert_eq!(ok.pairs().len(), 1);
    assert_eq!(
        serde_json::to_value(&ok)?,
        json!([{ "access_key_id": "AKID", "secret_access_key": "SECRET", "session_token": "TOKEN" }])
    );

    let error = error_of(AwsCredentialPairs::new(vec![pair("AKID", "MISSING", None)?], &bindings))?;
    assert_eq!(error, AwsCredentialPairsError::UnknownBinding(name("MISSING")?));
    assert_eq!(
        error.to_string(),
        "AWS credential pair names unknown secret binding 'MISSING'"
    );
    let error = error_of(AwsCredentialPairs::new(
        vec![pair("MISSING", "SECRET", None)?],
        &bindings,
    ))?;
    assert_eq!(error, AwsCredentialPairsError::UnknownBinding(name("MISSING")?));

    let error = error_of(AwsCredentialPairs::new(
        vec![pair("AKID", "SECRET", None)?, pair("TOKEN", "SECRET", None)?],
        &bindings,
    ))?;
    assert_eq!(error, AwsCredentialPairsError::DuplicateMember(name("SECRET")?));
    assert_eq!(
        error.to_string(),
        "AWS credential pair member 'SECRET' appears more than once"
    );

    let error = error_of(AwsCredentialPairs::new(vec![pair("AKID", "OTHER", None)?], &bindings))?;
    assert_eq!(
        error,
        AwsCredentialPairsError::HostsDiffer {
            access_key_id: name("AKID")?,
            member: name("OTHER")?,
        }
    );
    assert_eq!(
        error.to_string(),
        "AWS credential pair members 'AKID' and 'OTHER' inject at different hosts"
    );

    let mut scoped = parts("SCOPED")?;
    scoped.inject_hosts = hosts(&["a.example.com"])?;
    scoped.inject_scope.scope_version = InjectScopeVersion::V2;
    let mut extracted = parts("EXTRACTED")?;
    extracted.inject_hosts = hosts(&["a.example.com"])?;
    extracted.extract = Some(Extract::Regex(ExtractPattern::new("(a)")?));
    let mut denied = parts("DENIED")?;
    denied.inject_hosts = hosts(&["a.example.com"])?;
    denied.mode = SecretMode::Deny;
    let bindings = SecretBindings::new(vec![
        aws_binding("AKID", "a.example.com")?,
        SecretBinding::new(scoped)?,
        SecretBinding::new(extracted)?,
        SecretBinding::new(denied)?,
    ])?;
    let error = error_of(AwsCredentialPairs::new(vec![pair("AKID", "SCOPED", None)?], &bindings))?;
    assert!(matches!(error, AwsCredentialPairsError::HostsDiffer { .. }));
    for member in ["EXTRACTED", "DENIED"] {
        let error = error_of(AwsCredentialPairs::new(vec![pair("AKID", member, None)?], &bindings))?;
        assert_eq!(error, AwsCredentialPairsError::MemberNotWholeMask(name(member)?));
        assert_eq!(
            error.to_string(),
            format!("AWS credential pair member '{member}' must be a whole-value mask binding")
        );
    }
    Ok(())
}
