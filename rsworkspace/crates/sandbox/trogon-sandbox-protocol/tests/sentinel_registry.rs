mod common;

use common::{TestResult, host, hosts, name, pair};
use secrecy::ExposeSecret;
use trogon_sandbox_contract::{
    AwsCredentialPairsError, InjectMethod, InjectMethodSet, InjectPathPrefix, InjectScope, ResponseHeaderStrip,
    StrippedHeader,
};
use trogon_sandbox_protocol::{RealValue, RequestSite, SENTINEL_PREFIX, Sentinel, SentinelRegistry};

fn real(value: &str) -> RealValue {
    RealValue::new(value.to_string())
}

#[test]
fn sentinel_has_prefix_and_is_unique_per_mint() {
    let value = real("short");
    let first = Sentinel::mint(&value);
    let second = Sentinel::mint(&value);
    assert!(first.as_str().starts_with(SENTINEL_PREFIX));
    assert_ne!(first, second);
    assert_eq!(first.to_string(), first.as_str());
}

#[test]
fn sentinel_pads_to_the_real_length_and_never_shrinks() {
    let base = Sentinel::mint(&real("")).as_str().len();
    for extra in [0usize, 1, 7, 100, 4096] {
        let value = real(&"x".repeat(base + extra));
        let sentinel = Sentinel::mint(&value);
        assert_eq!(sentinel.as_str().len(), base + extra);
    }
    let sentinel = Sentinel::mint(&real("tiny"));
    assert_eq!(sentinel.as_str().len(), base);
}

#[test]
fn sentinel_padding_uses_the_restricted_alphabet() {
    let sentinel = Sentinel::mint(&real(&"x".repeat(400)));
    assert!(
        sentinel
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-')
    );
}

#[test]
fn real_value_debug_never_prints_the_value() {
    let value = real("super-secret-value");
    assert_eq!(format!("{value:?}"), "RealValue(***)");
    assert_eq!(value.expose(), "super-secret-value");
}

#[test]
fn real_value_converts_from_a_short_lived_token() {
    let token = trogon_sandbox_contract::ShortLivedToken::new("tok-123".to_string());
    assert_eq!(RealValue::from(token).expose(), "tok-123");
}

#[test]
fn register_is_idempotent_on_the_name_and_updates_in_place() -> TestResult {
    let mut registry = SentinelRegistry::new();
    assert!(registry.is_empty());
    let first = registry.register(name("tok")?, real("one-value"), hosts(&["a.example.com"])?);
    let second = registry.register(name("tok")?, real("two-value"), hosts(&["b.example.com"])?);
    assert_eq!(first, second);
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.lookup_real(&first).ok_or("missing")?.expose(), "two-value");
    assert_eq!(registry.sentinel_for(&name("tok")?), Some(&first));
    assert!(registry.names_injectable_at(&host("a.example.com")?, 443).is_empty());
    assert_eq!(registry.names_injectable_at(&host("b.example.com")?, 443).len(), 1);
    Ok(())
}

#[test]
fn distinct_names_get_distinct_sentinels() -> TestResult {
    let mut registry = SentinelRegistry::new();
    let first = registry.register(name("a")?, real("value-a"), hosts(&["x.com"])?);
    let second = registry.register(name("b")?, real("value-b"), hosts(&["x.com"])?);
    assert_ne!(first, second);
    assert_eq!(registry.pairs().count(), 2);
    assert!(registry.sentinel_for(&name("missing")?).is_none());
    Ok(())
}

#[test]
fn substitute_only_on_the_hosts_an_entry_names() -> TestResult {
    let mut registry = SentinelRegistry::new();
    let sentinel = registry.register(name("tok")?, real("real-token-value"), hosts(&["api.example.com"])?);
    let input = format!("Bearer {sentinel}");
    let allowed = registry.substitute(&input, &host("api.example.com")?, 443);
    assert_eq!(allowed.expose_secret(), "Bearer real-token-value");
    let other = registry.substitute(&input, &host("evil.example.com")?, 443);
    assert_eq!(other.expose_secret(), input);
    let untouched = registry.substitute("no sentinel here", &host("api.example.com")?, 443);
    assert_eq!(untouched.expose_secret(), "no sentinel here");
    Ok(())
}

#[test]
fn wildcards_and_ports_gate_injection() -> TestResult {
    let mut registry = SentinelRegistry::new();
    registry.register(name("wild")?, real("wild-value"), hosts(&["*.example.com"])?);
    registry.register(name("port")?, real("port-value"), hosts(&["svc.test:8443"])?);
    let wild = registry.names_injectable_at(&host("a.example.com")?, 443);
    assert_eq!(wild, vec![&name("wild")?]);
    assert!(registry.names_injectable_at(&host("example.com")?, 443).is_empty());
    assert_eq!(
        registry.names_injectable_at(&host("svc.test")?, 8443),
        vec![&name("port")?]
    );
    assert!(registry.names_injectable_at(&host("svc.test")?, 443).is_empty());
    Ok(())
}

#[test]
fn pairs_for_and_off_host_partition_the_entries() -> TestResult {
    let mut registry = SentinelRegistry::new();
    let on = registry.register(name("on")?, real("on-value"), hosts(&["a.com"])?);
    let off = registry.register(name("off")?, real("off-value"), hosts(&["b.com"])?);
    let target = host("a.com")?;
    let here: Vec<_> = registry
        .pairs_for_host(&target, 443)
        .map(|(sentinel, _)| sentinel)
        .collect();
    assert_eq!(here, vec![&on]);
    let away: Vec<_> = registry.pairs_off_host(&target, 443).collect();
    assert_eq!(away, vec![(&name("off")?, &off)]);
    Ok(())
}

#[test]
fn clear_drops_entries_pairs_and_strip_rules() -> TestResult {
    let mut registry = SentinelRegistry::new();
    registry.register(name("a")?, real("value-a"), hosts(&["x.com"])?);
    registry.register(name("b")?, real("value-b"), hosts(&["x.com"])?);
    registry.link_aws_pair(&pair("a", "b", None)?)?;
    registry.register_strip(
        hosts(&["x.com"])?,
        ResponseHeaderStrip::new([StrippedHeader::new("set-cookie")?]),
    );
    registry.clear();
    assert!(registry.is_empty());
    assert_eq!(registry.aws_pairs_for_host(&host("x.com")?, 443).count(), 0);
    assert!(registry.strip_for_host(&host("x.com")?, 443).is_empty());
    Ok(())
}

fn scoped(prefix: Option<&str>, methods: Option<Vec<InjectMethod>>) -> Result<InjectScope, Box<dyn std::error::Error>> {
    Ok(InjectScope {
        path_prefix: prefix.map(InjectPathPrefix::new).transpose()?,
        methods: methods.map(InjectMethodSet::new).transpose()?,
        ..InjectScope::default()
    })
}

#[test]
fn scoped_entries_fail_closed_without_a_request_site() -> TestResult {
    let mut registry = SentinelRegistry::new();
    let sentinel = registry.register(name("tok")?, real("scoped-value"), hosts(&["api.com"])?);
    registry.set_scope(&name("tok")?, &scoped(Some("/v1"), Some(vec![InjectMethod::Get]))?);
    let target = host("api.com")?;
    assert_eq!(registry.names_injectable_at(&target, 443).len(), 1);
    assert!(registry.names_injectable_for(&target, 443, None).is_empty());
    let get_v1 = RequestSite {
        method: "GET",
        path: "/v1/items",
    };
    let post_v1 = RequestSite {
        method: "POST",
        path: "/v1/items",
    };
    let get_v2 = RequestSite {
        method: "GET",
        path: "/v2/items",
    };
    assert_eq!(registry.names_injectable_for(&target, 443, Some(get_v1)).len(), 1);
    assert!(registry.names_injectable_for(&target, 443, Some(post_v1)).is_empty());
    assert!(registry.names_injectable_for(&target, 443, Some(get_v2)).is_empty());
    assert_eq!(registry.pairs_for_request(&target, 443, Some(get_v1)).count(), 1);
    assert_eq!(registry.pairs_for_host(&target, 443).count(), 0);
    assert_eq!(registry.pairs_off_request(&target, 443, Some(post_v1)).count(), 1);
    assert_eq!(
        registry.lookup_real(&sentinel).ok_or("missing")?.expose(),
        "scoped-value"
    );
    Ok(())
}

fn aws_registry() -> Result<SentinelRegistry, Box<dyn std::error::Error>> {
    let mut registry = SentinelRegistry::new();
    registry.register(name("id")?, real("AKIAEXAMPLE"), hosts(&["aws.example.com"])?);
    registry.register(name("secret")?, real("secret-key-value"), hosts(&["aws.example.com"])?);
    registry.register(
        name("token")?,
        real("session-token-value"),
        hosts(&["aws.example.com"])?,
    );
    registry.link_aws_pair(&pair("id", "secret", Some("token"))?)?;
    Ok(registry)
}

#[test]
fn aws_pair_resolves_only_at_the_inject_hosts() -> TestResult {
    let registry = aws_registry()?;
    let target = host("aws.example.com")?;
    let found: Vec<_> = registry.aws_pairs_for_host(&target, 443).collect();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].access_key_id.expose(), "AKIAEXAMPLE");
    assert_eq!(found[0].secret_access_key.expose(), "secret-key-value");
    assert_eq!(found[0].session_token.ok_or("token")?.expose(), "session-token-value");
    assert_eq!(found[0].binding, &name("id")?);
    assert_eq!(registry.aws_pairs_for_host(&host("other.com")?, 443).count(), 0);
    assert!(format!("{:?}", found[0]).contains("RealValue(***)"));
    Ok(())
}

#[test]
fn aws_pair_without_session_token_resolves() -> TestResult {
    let mut registry = SentinelRegistry::new();
    registry.register(name("id")?, real("AKIAEXAMPLE"), hosts(&["aws.com"])?);
    registry.register(name("secret")?, real("secret-key-value"), hosts(&["aws.com"])?);
    registry.link_aws_pair(&pair("id", "secret", None)?)?;
    let target = host("aws.com")?;
    let found: Vec<_> = registry.aws_pairs_for_host(&target, 443).collect();
    assert_eq!(found.len(), 1);
    assert!(found[0].session_token.is_none());
    Ok(())
}

#[test]
fn linking_an_unknown_member_is_refused() -> TestResult {
    let mut registry = SentinelRegistry::new();
    registry.register(name("id")?, real("AKIAEXAMPLE"), hosts(&["aws.com"])?);
    let error = registry
        .link_aws_pair(&pair("id", "ghost", None)?)
        .err()
        .ok_or("expected error")?;
    assert!(matches!(error, AwsCredentialPairsError::UnknownBinding(_)));
    Ok(())
}

#[test]
fn relinking_a_pair_replaces_the_previous_link() -> TestResult {
    let mut registry = aws_registry()?;
    registry.link_aws_pair(&pair("id", "secret", None)?)?;
    let target = host("aws.example.com")?;
    let found: Vec<_> = registry.aws_pairs_for_host(&target, 443).collect();
    assert_eq!(found.len(), 1);
    assert!(found[0].session_token.is_none());
    Ok(())
}

#[test]
fn strips_union_at_matching_hosts_only() -> TestResult {
    let mut registry = SentinelRegistry::new();
    registry.register_strip(
        hosts(&["a.example.com"])?,
        ResponseHeaderStrip::new([StrippedHeader::new("set-cookie")?]),
    );
    registry.register_strip(
        hosts(&["*.example.com"])?,
        ResponseHeaderStrip::new([StrippedHeader::new("x-token")?]),
    );
    registry.register_strip(hosts(&["a.example.com"])?, ResponseHeaderStrip::default());
    let a = registry.strip_for_host(&host("a.example.com")?, 443);
    let names: Vec<&str> = a.headers().iter().map(StrippedHeader::as_str).collect();
    assert!(names.contains(&"set-cookie") && names.contains(&"x-token"));
    let b = registry.strip_for_host(&host("b.example.com")?, 443);
    assert_eq!(b.headers().len(), 1);
    assert!(registry.strip_for_host(&host("c.org")?, 443).is_empty());
    Ok(())
}

#[test]
fn the_debug_form_lists_names_and_sentinels_but_never_real_values() -> TestResult {
    let mut registry = SentinelRegistry::new();
    let sentinel = registry.register(
        name("api-key")?,
        real("super-secret-value"),
        hosts(&["api.example.com"])?,
    );
    let rendered = format!("{registry:?}");
    assert!(rendered.contains("api-key"), "{rendered}");
    assert!(rendered.contains(sentinel.as_str()), "{rendered}");
    assert!(!rendered.contains("super-secret-value"), "{rendered}");
    Ok(())
}

#[test]
fn a_sentinel_is_found_inside_other_text_whatever_its_case() -> TestResult {
    let mut registry = SentinelRegistry::new();
    let sentinel = registry.register(name("api")?, real("sk-real-value"), hosts(&["api.example.com"])?);
    assert!(Sentinel::appears_in(&format!(
        "x-{}.attacker.example",
        sentinel.as_str()
    )));
    assert!(Sentinel::appears_in(&format!(
        "{}.attacker.example",
        sentinel.as_str().to_ascii_uppercase()
    )));
    assert!(Sentinel::appears_in("eyJhbGciOiJub25lIn0.e30.ZmFrZS1zaWduYXR1cmU"));
    assert!(Sentinel::appears_in(
        "eyjhbgcioijub25lin0.e30.zmfrzs1zawduyxr1cmu.attacker.example"
    ));
    assert!(!Sentinel::appears_in("api.example.com"));
    assert!(!Sentinel::appears_in("fake-value.example.com"));
    Ok(())
}
