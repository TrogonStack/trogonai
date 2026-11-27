mod common;

use common::{TestResult, host, hosts, name, pair};
use secrecy::ExposeSecret;
use trogon_sandbox_contract::{
    InjectMethod, InjectMethodSet, InjectPathPrefix, InjectScope, JwtMasking, ResponseHeaderStrip, StrippedHeader,
};
use trogon_sandbox_protocol::{HandoffError, RealValue, RegistryHandoff, RequestSite, SentinelRegistry};

fn registry() -> Result<SentinelRegistry, Box<dyn std::error::Error>> {
    let mut registry = SentinelRegistry::new();
    registry.register(
        name("gh")?,
        RealValue::new("ghp_supersecret".to_string()),
        hosts(&["api.github.com"])?,
    );
    Ok(registry)
}

#[test]
fn round_trip_keeps_sentinels_and_real_values() -> TestResult {
    let original = registry()?;
    let secret = name("gh")?;
    let sentinel = original.sentinel_for(&secret).ok_or("sentinel")?.clone();
    let restored = RegistryHandoff::from_registry(&original)?.into_registry()?;
    assert_eq!(restored.sentinel_for(&secret), Some(&sentinel));
    assert_eq!(
        restored.lookup_real(&sentinel).ok_or("real")?.expose(),
        "ghp_supersecret"
    );
    let substituted = restored.substitute(sentinel.as_str(), &host("api.github.com")?, 443);
    assert_eq!(substituted.expose_secret(), "ghp_supersecret");
    Ok(())
}

#[test]
fn the_wire_form_round_trips_through_from_wire() -> TestResult {
    let handoff = RegistryHandoff::from_registry(&registry()?)?;
    let wire = handoff.expose_wire().to_string();
    let restored = RegistryHandoff::from_wire(wire).into_registry()?;
    assert_eq!(restored.len(), 1);
    Ok(())
}

#[test]
fn scopes_pairs_strips_and_jwt_entries_survive_the_handoff() -> TestResult {
    let mut original = SentinelRegistry::new();
    let hosts_aws = hosts(&["aws.example.com"])?;
    original.register(
        name("id")?,
        RealValue::new("AKIAEXAMPLE".to_string()),
        hosts_aws.clone(),
    );
    original.register(
        name("secret")?,
        RealValue::new("secret-key-value".to_string()),
        hosts_aws,
    );
    original.link_aws_pair(&pair("id", "secret", None)?)?;
    original.register_strip(
        hosts(&["aws.example.com"])?,
        ResponseHeaderStrip::new([StrippedHeader::new("set-cookie")?]),
    );
    let header = "eyJhbGciOiJIUzI1NiJ9";
    let body = "eyJzdWIiOiJhbGljZSJ9";
    let jwt = format!("{header}.{body}.c2ln");
    let masking = JwtMasking {
        mask_claims: vec![trogon_sandbox_contract::ClaimName::new("sub")?],
        pattern: None,
    };
    original.register_jwt(
        name("tok")?,
        &masking,
        RealValue::new(jwt),
        hosts(&["api.example.com"])?,
    )?;
    original.set_scope(
        &name("tok")?,
        &InjectScope {
            path_prefix: Some(InjectPathPrefix::new("/v1")?),
            methods: Some(InjectMethodSet::new(vec![InjectMethod::Get])?),
            ..InjectScope::default()
        },
    );
    let restored = RegistryHandoff::from_registry(&original)?.into_registry()?;
    assert_eq!(restored.len(), original.len());
    let aws = host("aws.example.com")?;
    assert_eq!(restored.aws_pairs_for_host(&aws, 443).count(), 1);
    assert_eq!(restored.strip_for_host(&aws, 443).headers().len(), 1);
    let api = host("api.example.com")?;
    assert!(restored.names_injectable_for(&api, 443, None).is_empty());
    let site = RequestSite {
        method: "GET",
        path: "/v1/x",
    };
    assert_eq!(restored.names_injectable_for(&api, 443, Some(site)).len(), 2);
    Ok(())
}

#[test]
fn debug_never_shows_the_real_value() -> TestResult {
    let handoff = RegistryHandoff::from_registry(&registry()?)?;
    assert!(!format!("{handoff:?}").contains("supersecret"));
    Ok(())
}

#[test]
fn malformed_handoff_error_carries_no_content() -> TestResult {
    let error = RegistryHandoff::from_wire(r#"[{"real":"ghp_supersecret""#.to_string())
        .into_registry()
        .err()
        .ok_or("expected error")?;
    assert!(!format!("{error} {error:?}").contains("supersecret"));
    Ok(())
}

#[test]
fn inconsistent_handoffs_are_refused() -> TestResult {
    let cases = [
        r#"{"entries":[{"name":"gh","sentinel":"fake_value_X Y","real":"r","inject_hosts":["a.com"]}]}"#,
        r#"{"entries":[{"name":"gh","sentinel":"not-a-sentinel","real":"r","inject_hosts":["a.com"]}]}"#,
        concat!(
            r#"{"entries":[{"name":"gh","sentinel":"fake_value_a","real":"r","inject_hosts":["a.com"]},"#,
            r#"{"name":"gh","sentinel":"fake_value_b","real":"r","inject_hosts":["a.com"]}]}"#
        ),
        concat!(
            r#"{"entries":[{"name":"gh","sentinel":"fake_value_a","real":"r","inject_hosts":["a.com"]}],"#,
            r#""aws_pairs":[{"access_key_id":"gh","secret_access_key":"ghost"}]}"#
        ),
    ];
    for wire in cases {
        let error = RegistryHandoff::from_wire(wire.to_string())
            .into_registry()
            .err()
            .ok_or("expected error")?;
        assert_eq!(error, HandoffError::Malformed, "{wire}");
    }
    assert!(!HandoffError::Unencodable.to_string().is_empty());
    assert!(!HandoffError::Malformed.to_string().is_empty());
    Ok(())
}
