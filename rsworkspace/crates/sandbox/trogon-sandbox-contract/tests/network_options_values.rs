use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    AllowedTlsTermination, AmbiguousTargets, CaFiles, DeniedDomain, DeniedDomainReasons, DenyReason, DenyReasonError,
    DomainOnlyAllows, DomainPattern, ExternalProxy, ExtraCaCertPaths, IpRange, LiveFlowResweep, LoopbackNames,
    MitmUnixSocket, NamesMatchedByName, NetworkOptions, PlaintextHeaderSet, Port, PrivateRangesByName,
    ReasonedRefusals, RequestBodyCap, RequestBodyCapError, TlsExclusions, TlsInterceptionExemptions, TunnelCap,
    TunnelCapError, UnlistedDestinations, UpstreamCaCertificates,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn rejects<T: DeserializeOwned>(value: serde_json::Value) -> TestResult {
    let from_value = error_of(serde_json::from_value::<T>(value.clone()))?;
    assert_eq!(from_value.classify(), Data);
    let from_str = error_of(serde_json::from_str::<T>(&value.to_string()))?;
    assert_eq!(from_str.classify(), Data);
    Ok(())
}

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T, json: serde_json::Value) -> TestResult {
    assert_eq!(serde_json::to_value(value)?, json);
    assert_eq!(&serde_json::from_value::<T>(json.clone())?, value);
    assert_eq!(&serde_json::from_str::<T>(&json.to_string())?, value);
    Ok(())
}

fn port(value: u16) -> Result<Port, Box<dyn std::error::Error>> {
    Ok(Port::new(value)?)
}

fn pattern(value: &str) -> Result<DomainPattern, Box<dyn std::error::Error>> {
    Ok(DomainPattern::new(value)?)
}

#[test]
fn a_deny_reason_is_cleaned_for_a_status_line() -> TestResult {
    let reason = DenyReason::new("host is on the deny list")?;
    assert_eq!(reason.as_str(), "host is on the deny list");
    assert_eq!(reason.to_string(), "host is on the deny list");
    assert_eq!(String::from(reason.clone()), "host is on the deny list");
    assert_eq!(DenyReason::try_from("host is on the deny list".to_string())?, reason);
    round_trip(&reason, json!("host is on the deny list"))?;

    assert_eq!(DenyReason::new("a\u{0}\u{1}\u{202e}b")?.as_str(), "a b");
    assert_eq!(DenyReason::new("a\r\nb")?.as_str(), "a b");
    assert_eq!(DenyReason::new("<b>no</b>")?.as_str(), "bno/b");
    assert_eq!(DenyReason::new("  padded  ")?.as_str(), "padded");
    assert_eq!(serde_json::from_value::<DenyReason>(json!("x\u{7f}y"))?.as_str(), "x y");
    Ok(())
}

#[test]
fn a_deny_reason_replaces_every_display_unsafe_character() -> TestResult {
    for unsafe_char in [
        '\u{0}',
        '\u{7f}',
        '\u{9f}',
        '\u{ad}',
        '\u{61c}',
        '\u{200b}',
        '\u{2028}',
        '\u{2029}',
        '\u{202a}',
        '\u{2060}',
        '\u{2066}',
        '\u{feff}',
        '\u{e0001}',
    ] {
        let raw = format!("a{unsafe_char}b");
        assert_eq!(DenyReason::new(&raw)?.as_str(), "a b");
    }
    Ok(())
}

#[test]
fn a_deny_reason_keeps_at_most_500_utf16_units() -> TestResult {
    assert_eq!(DenyReason::MAX_UTF16_UNITS, 500);
    let long = "a".repeat(600);
    assert_eq!(DenyReason::new(&long)?.as_str().len(), 500);

    let straddling = format!("{}\u{1f600}", "a".repeat(499));
    assert_eq!(DenyReason::new(&straddling)?.as_str(), "a".repeat(499));

    let trailing_space = format!("{} b", "a".repeat(499));
    assert_eq!(DenyReason::new(&trailing_space)?.as_str(), "a".repeat(499));
    Ok(())
}

#[test]
fn a_deny_reason_with_nothing_left_is_rejected() -> TestResult {
    for raw in ["", "   ", "<>", "\u{0}\u{1}"] {
        let error = error_of(DenyReason::new(raw))?;
        assert_eq!(error, DenyReasonError::Empty);
        assert_eq!(error.to_string(), "a deny reason cannot be empty");
        assert_eq!(error_of(DenyReason::try_from(raw.to_string()))?, DenyReasonError::Empty);
        rejects::<DenyReason>(json!(raw))?;
    }
    rejects::<DenyReason>(json!(7))?;
    Ok(())
}

#[test]
fn denied_domain_reasons_key_a_reason_by_entry() -> TestResult {
    let evil = DeniedDomain::new("*.evil.example")?;
    let any = DeniedDomain::new("*")?;
    let reasons = DeniedDomainReasons::new([
        (evil.clone(), DenyReason::new("known bad")?),
        (any.clone(), DenyReason::new("closed")?),
    ]);
    assert!(!reasons.is_empty());
    assert_eq!(reasons.get(&evil), Some(&DenyReason::new("known bad")?));
    assert_eq!(reasons.get(&DeniedDomain::new("other.example")?), None);
    assert_eq!(reasons.iter().count(), 2);
    round_trip(&reasons, json!({"*": "closed", "*.evil.example": "known bad"}))?;

    let empty = DeniedDomainReasons::default();
    assert!(empty.is_empty());
    round_trip(&empty, json!({}))?;
    Ok(())
}

#[test]
fn denied_domain_reasons_drop_keys_that_are_no_entry() -> TestResult {
    let reasons = serde_json::from_value::<DeniedDomainReasons>(json!({
        "*.evil.example": "known bad",
        "not a domain!": "dropped",
    }))?;
    assert_eq!(reasons.iter().count(), 1);
    assert_eq!(
        reasons.get(&DeniedDomain::new("*.evil.example")?),
        Some(&DenyReason::new("known bad")?)
    );
    let from_map = DeniedDomainReasons::from(BTreeMap::from([("*".to_string(), DenyReason::new("closed")?)]));
    assert_eq!(from_map.iter().count(), 1);
    rejects::<DeniedDomainReasons>(json!({"*": ""}))?;
    rejects::<DeniedDomainReasons>(json!(["*"]))?;
    Ok(())
}

#[test]
fn unlisted_destinations_use_snake_case_strings() -> TestResult {
    assert_eq!(UnlistedDestinations::default(), UnlistedDestinations::Consult);
    round_trip(&UnlistedDestinations::Consult, json!("consult"))?;
    round_trip(&UnlistedDestinations::Refuse, json!("refuse"))?;
    rejects::<UnlistedDestinations>(json!("Refuse"))?;
    Ok(())
}

#[test]
fn a_tunnel_cap_is_one_to_65536() -> TestResult {
    assert_eq!(TunnelCap::MAX, 65_536);
    let cap = TunnelCap::new(256)?;
    assert_eq!(cap.get(), 256);
    assert_eq!(u32::from(cap), 256);
    assert_eq!(TunnelCap::try_from(256)?, cap);
    assert_eq!(TunnelCap::new(1)?.get(), 1);
    assert_eq!(TunnelCap::new(65_536)?.get(), 65_536);
    round_trip(&cap, json!(256))?;

    for bad in [0, 65_537] {
        let error = error_of(TunnelCap::new(bad))?;
        assert_eq!(error, TunnelCapError::OutOfRange { max: 65_536 });
        assert_eq!(error.to_string(), "the tunnel cap must be between 1 and 65536");
        assert_eq!(error_of(TunnelCap::try_from(bad))?, error);
        rejects::<TunnelCap>(json!(bad))?;
    }
    Ok(())
}

#[test]
fn a_mitm_unix_socket_lists_its_socket_and_domains() -> TestResult {
    let socket = MitmUnixSocket {
        socket_path: PathBuf::from("/run/mitm.sock"),
        domains: vec![pattern("example.com")?, pattern("*.example.org:443")?],
    };
    round_trip(
        &socket,
        json!({"socket_path": "/run/mitm.sock", "domains": ["example.com", "*.example.org:443"]}),
    )?;
    rejects::<MitmUnixSocket>(json!({"socket_path": "/run/mitm.sock", "domains": ["*"]}))?;
    Ok(())
}

#[test]
fn upstream_ca_certificates_are_a_json_array() -> TestResult {
    let certificates = UpstreamCaCertificates::new(["pem one".to_string(), "pem two".to_string()]);
    assert!(!certificates.is_empty());
    assert_eq!(certificates.iter().count(), 2);
    round_trip(&certificates, json!(["pem one", "pem two"]))?;
    assert!(UpstreamCaCertificates::default().is_empty());
    rejects::<UpstreamCaCertificates>(json!("pem"))?;
    Ok(())
}

macro_rules! flag_test {
    ($name:ident, $flag:ident) => {
        #[test]
        fn $name() -> TestResult {
            assert_eq!($flag::default(), $flag::Disabled);
            assert!(!$flag::Disabled.is_enabled());
            assert!($flag::Enabled.is_enabled());
            assert_eq!($flag::from(true), $flag::Enabled);
            assert_eq!($flag::from(false), $flag::Disabled);
            assert!(bool::from($flag::Enabled));
            assert!(!bool::from($flag::Disabled));
            round_trip(&$flag::Enabled, json!(true))?;
            round_trip(&$flag::Disabled, json!(false))?;
            rejects::<$flag>(json!("true"))?;
            Ok(())
        }
    };
}

flag_test!(loopback_names_is_a_json_boolean, LoopbackNames);
flag_test!(reasoned_refusals_is_a_json_boolean, ReasonedRefusals);
flag_test!(allowed_tls_termination_is_a_json_boolean, AllowedTlsTermination);
flag_test!(plaintext_header_set_is_a_json_boolean, PlaintextHeaderSet);
flag_test!(private_ranges_by_name_is_a_json_boolean, PrivateRangesByName);
flag_test!(names_matched_by_name_is_a_json_boolean, NamesMatchedByName);
flag_test!(live_flow_resweep_is_a_json_boolean, LiveFlowResweep);

#[test]
fn a_request_body_cap_is_at_least_one_byte() -> TestResult {
    assert_eq!(RequestBodyCap::new(1024)?.bytes(), 1024);
    let error = error_of(RequestBodyCap::new(0))?;
    assert_eq!(error, RequestBodyCapError::Zero);
    assert_eq!(error.to_string(), "the request body cap must be at least 1 byte");
    Ok(())
}

#[test]
fn file_and_pattern_lists_expose_what_they_hold() -> TestResult {
    let paths = ExtraCaCertPaths::new([PathBuf::from("/etc/a.pem"), PathBuf::from("/etc/b.pem")]);
    assert!(!paths.is_empty());
    assert_eq!(paths.as_slice().len(), 2);
    assert!(ExtraCaCertPaths::default().is_empty());

    let files = CaFiles::new(PathBuf::from("/ca/cert.pem"), PathBuf::from("/ca/key.pem"));
    assert_eq!(files.cert_path(), PathBuf::from("/ca/cert.pem"));
    assert_eq!(files.key_path(), PathBuf::from("/ca/key.pem"));

    let exclusions = TlsExclusions::new([pattern("pinned.example.com")?]);
    assert!(!exclusions.is_empty());
    assert_eq!(exclusions.as_slice(), [pattern("pinned.example.com")?]);
    assert!(TlsExclusions::default().is_empty());

    let exemptions = TlsInterceptionExemptions::new([pattern("exempt.example.com")?], [IpRange::new("10.0.0.0/8")?]);
    assert!(!exemptions.is_empty());
    assert_eq!(exemptions.hostnames(), [pattern("exempt.example.com")?]);
    assert_eq!(exemptions.addresses(), [IpRange::new("10.0.0.0/8")?]);
    assert!(TlsInterceptionExemptions::default().is_empty());
    assert!(!TlsInterceptionExemptions::new([pattern("exempt.example.com")?], []).is_empty());
    assert!(!TlsInterceptionExemptions::new([], [IpRange::new("10.0.0.0/8")?]).is_empty());
    Ok(())
}

#[test]
fn an_external_proxy_covers_the_sides_it_names() -> TestResult {
    let http = ExternalProxy::http_only(port(8080)?);
    assert_eq!((http.http_port(), http.socks_port()), (Some(port(8080)?), None));
    let socks = ExternalProxy::socks_only(port(1080)?);
    assert_eq!((socks.http_port(), socks.socks_port()), (None, Some(port(1080)?)));
    let both = ExternalProxy::both(port(8080)?, port(1080)?);
    assert_eq!(
        (both.http_port(), both.socks_port()),
        (Some(port(8080)?), Some(port(1080)?))
    );
    Ok(())
}

#[test]
fn the_remaining_modes_default_to_the_cautious_choice() {
    assert_eq!(AmbiguousTargets::default(), AmbiguousTargets::Refuse);
    assert_ne!(AmbiguousTargets::AsSpelled, AmbiguousTargets::Normalize);
    assert_eq!(DomainOnlyAllows::default(), DomainOnlyAllows::PassThrough);
    assert_ne!(DomainOnlyAllows::default(), DomainOnlyAllows::RequireTermination);
}

#[test]
fn network_options_start_with_everything_off() -> TestResult {
    let defaults = NetworkOptions::default();
    assert_eq!(defaults.ca_files, None);
    assert!(defaults.tls_exclusions.is_empty());
    assert_eq!(defaults.unlisted, UnlistedDestinations::Consult);
    assert_eq!(defaults.loopback_names, LoopbackNames::Disabled);
    assert_eq!(defaults.ambiguous_targets, AmbiguousTargets::Refuse);
    assert_eq!(defaults.domain_only_allows, DomainOnlyAllows::PassThrough);
    assert_eq!(defaults.handshake_timeout, None);

    let options = NetworkOptions {
        tunnel_cap: Some(TunnelCap::new(8)?),
        request_body_cap: Some(RequestBodyCap::new(4096)?),
        handshake_timeout: Some(Duration::from_secs(5)),
        binding_ttl: Some(Duration::from_secs(60)),
        revalidate_interval: Some(Duration::from_secs(30)),
        denied_resolved_addresses: vec![IpRange::new("169.254.0.0/16")?],
        reasoned_refusals: ReasonedRefusals::Enabled,
        ..NetworkOptions::default()
    };
    assert_eq!(options.clone(), options);
    assert_ne!(options, defaults);
    Ok(())
}
