use std::collections::BTreeMap;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{
    AddressDenialClass, Base64Data, BodyTruncation, DecideMethod, DecideRequest, DecideResponse, EgressDecision,
    EgressViolation, EgressViolationReason, FsOperation, FsViolation, Host, HttpMethod, LeaseId, LiteralPath,
    LiteralPathError, Port, ProxyUid, RefusedRequest, RequestDecideBody, RequestDecideRequest, RequestDecideResponse,
    SecretName, SecretNameError, SocketMark, UnsupportedSigv4Shape, VerdictScope,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn rejects<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> TestResult {
    assert_eq!(error_of(serde_json::from_value::<T>(value.clone()))?.classify(), Data);
    assert_eq!(
        error_of(serde_json::from_str::<T>(&value.to_string()))?.classify(),
        Data
    );
    Ok(())
}

fn violation() -> EgressViolation {
    EgressViolation {
        lease_id: LeaseId::new("lease-1"),
        host: Some("example.com".to_string()),
        port: 443,
        dest_ip: None,
        decision: EgressDecision::Deny,
        reason: EgressViolationReason::PolicyDenied,
        binding: None,
        request: None,
        at: UNIX_EPOCH + Duration::new(10, 5),
    }
}

#[test]
fn a_minimal_violation_has_an_exact_wire_shape() -> TestResult {
    let wire = json!({
        "lease_id": "lease-1",
        "host": "example.com",
        "port": 443,
        "dest_ip": null,
        "decision": "deny",
        "reason": "policy_denied",
        "at": {"secs_since_epoch": 10, "nanos_since_epoch": 5},
    });
    assert_eq!(serde_json::to_value(violation())?, wire);
    assert_eq!(serde_json::from_value::<EgressViolation>(wire.clone())?, violation());
    assert_eq!(serde_json::from_str::<EgressViolation>(&wire.to_string())?, violation());
    Ok(())
}

#[test]
fn a_full_violation_round_trips() -> TestResult {
    let full = EgressViolation {
        dest_ip: Some("169.254.169.254".parse()?),
        reason: EgressViolationReason::DestinationAddressDenied(AddressDenialClass::LinkLocal),
        binding: Some(SecretName::new("api-key")?),
        request: Some(RefusedRequest::new("GET", "https://example.com/a?x=1", "denied")),
        ..violation()
    };
    let wire = json!({
        "lease_id": "lease-1",
        "host": "example.com",
        "port": 443,
        "dest_ip": "169.254.169.254",
        "decision": "deny",
        "reason": {"destination_address_denied": "link_local"},
        "binding": "api-key",
        "request": {"method": "GET", "target": "https://example.com/a?\u{2026}", "reason": "denied"},
        "at": {"secs_since_epoch": 10, "nanos_since_epoch": 5},
    });
    assert_eq!(serde_json::to_value(&full)?, wire);
    assert_eq!(serde_json::from_value::<EgressViolation>(wire.clone())?, full);
    assert_eq!(serde_json::from_str::<EgressViolation>(&wire.to_string())?, full);
    Ok(())
}

#[test]
fn a_violation_rejects_an_invalid_binding_name() -> TestResult {
    rejects::<EgressViolation>(json!({
        "lease_id": "lease-1",
        "host": null,
        "port": 1,
        "dest_ip": null,
        "decision": "deny",
        "reason": "policy_denied",
        "binding": "bad name",
        "at": {"secs_since_epoch": 0, "nanos_since_epoch": 0},
    }))
}

#[test]
fn every_reason_has_a_snake_case_name() -> TestResult {
    let cases = [
        (EgressViolationReason::PolicyDenied, json!("policy_denied")),
        (
            EgressViolationReason::GssEncryptionUnsupported,
            json!("gss_encryption_unsupported"),
        ),
        (
            EgressViolationReason::PostgresTlsDeclined,
            json!("postgres_tls_declined"),
        ),
        (
            EgressViolationReason::SecretSentinelOffHost,
            json!("secret_sentinel_off_host"),
        ),
        (
            EgressViolationReason::SecretCompressedResponse,
            json!("secret_compressed_response"),
        ),
        (
            EgressViolationReason::SniDestinationMismatch,
            json!("sni_destination_mismatch"),
        ),
        (
            EgressViolationReason::SniResolutionFailed,
            json!("sni_resolution_failed"),
        ),
        (EgressViolationReason::DeciderDenied, json!("decider_denied")),
        (EgressViolationReason::DeciderTimedOut, json!("decider_timed_out")),
        (EgressViolationReason::DeciderUnavailable, json!("decider_unavailable")),
        (
            EgressViolationReason::DeciderMalformedReply,
            json!("decider_malformed_reply"),
        ),
        (EgressViolationReason::DeciderOverloaded, json!("decider_overloaded")),
        (
            EgressViolationReason::SecretSigv4Unsignable,
            json!("secret_sigv4_unsignable"),
        ),
        (EgressViolationReason::SecretJwtMalformed, json!("secret_jwt_malformed")),
        (
            EgressViolationReason::SecretJwtClaimsUnmatched,
            json!("secret_jwt_claims_unmatched"),
        ),
        (
            EgressViolationReason::OpaqueTunnelRefused,
            json!("opaque_tunnel_refused"),
        ),
        (EgressViolationReason::RequestDenied, json!("request_denied")),
        (EgressViolationReason::TunnelLimitReached, json!("tunnel_limit_reached")),
        (EgressViolationReason::MalformedHost, json!("malformed_host")),
        (EgressViolationReason::PolicyUnavailable, json!("policy_unavailable")),
        (
            EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Streaming),
            json!({"secret_sigv4_unsupported": "streaming"}),
        ),
        (
            EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Presigned),
            json!({"secret_sigv4_unsupported": "presigned"}),
        ),
        (
            EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Sigv4a),
            json!({"secret_sigv4_unsupported": "sigv4a"}),
        ),
    ];
    for (reason, wire) in cases {
        assert_eq!(serde_json::to_value(reason)?, wire);
        assert_eq!(serde_json::from_value::<EgressViolationReason>(wire)?, reason);
    }
    rejects::<EgressViolationReason>(json!("nope"))?;
    rejects::<EgressViolationReason>(json!({"destination_address_denied": "nope"}))
}

#[test]
fn address_denial_classes_serialize_and_order() -> TestResult {
    let classes = [
        (AddressDenialClass::DenyListed, "deny_listed"),
        (AddressDenialClass::Loopback, "loopback"),
        (AddressDenialClass::Unspecified, "unspecified"),
        (AddressDenialClass::LinkLocal, "link_local"),
        (AddressDenialClass::CloudMetadata, "cloud_metadata"),
        (AddressDenialClass::Multicast, "multicast"),
        (AddressDenialClass::Broadcast, "broadcast"),
        (AddressDenialClass::OwnInterface, "own_interface"),
        (AddressDenialClass::PrivateUse, "private_use"),
        (AddressDenialClass::Listed, "listed"),
        (AddressDenialClass::NonLoopback, "non_loopback"),
    ];
    for (class, name) in classes {
        assert_eq!(serde_json::to_value(class)?, json!(name));
        assert_eq!(serde_json::from_value::<AddressDenialClass>(json!(name))?, class);
    }
    assert!(classes.windows(2).all(|pair| pair[0].0 < pair[1].0));
    Ok(())
}

#[test]
fn a_refused_request_cuts_the_query_and_fragment() {
    let cases = [
        ("https://a.test/p", "https://a.test/p"),
        ("https://a.test/p?q=1", "https://a.test/p?\u{2026}"),
        ("https://a.test/p#frag", "https://a.test/p#\u{2026}"),
        ("https://a.test/p?q=1#f", "https://a.test/p?\u{2026}"),
    ];
    for (url, target) in cases {
        let refused = RefusedRequest::new("POST", url, "why");
        assert_eq!(refused.method(), "POST");
        assert_eq!(refused.target(), target);
        assert_eq!(refused.reason(), "why");
    }
}

#[test]
fn a_refused_request_is_cut_on_the_wire_too() -> TestResult {
    let wire = json!({"method": "GET", "target": "https://a.test/p?token=secret#f", "reason": "why"});
    let read = serde_json::from_value::<RefusedRequest>(wire.clone())?;
    assert_eq!(read.target(), "https://a.test/p?\u{2026}");
    assert_eq!(
        read,
        RefusedRequest::new("GET", "https://a.test/p?token=secret#f", "why")
    );
    assert_eq!(
        serde_json::to_value(&read)?,
        json!({"method": "GET", "target": "https://a.test/p?\u{2026}", "reason": "why"})
    );
    assert_eq!(serde_json::from_str::<RefusedRequest>(&wire.to_string())?, read);
    rejects::<RefusedRequest>(json!({"method": "GET"}))
}

#[test]
fn a_body_is_standard_base64() -> TestResult {
    let data = Base64Data::new("eHl6")?;
    assert_eq!(data.as_str(), "eHl6");
    assert_eq!(data.to_string(), "eHl6");
    assert_eq!("eHl6".parse::<Base64Data>()?, data);
    assert_eq!(Base64Data::try_from("eHl6".to_string())?, data);
    assert_eq!(String::from(data), "eHl6");
    assert_eq!(Base64Data::new("")?.as_str(), "");
    for input in ["a", "eHl6=", "e Hl6", "eHl-", "YQ"] {
        let error = error_of(Base64Data::new(input))?;
        assert_eq!(
            error.to_string(),
            "value must be standard base64 with padding",
            "{input:?}"
        );
        assert!(std::error::Error::source(&error).is_some(), "{input:?}");
        rejects::<RequestDecideBody>(json!({"data": input}))?;
    }
    Ok(())
}

#[test]
fn a_filesystem_violation_canonicalizes_its_path() -> TestResult {
    let wire = json!({
        "lease_id": "lease-1",
        "path": "/etc/./a//b/../passwd",
        "operation": "set_metadata",
        "at": {"secs_since_epoch": 1, "nanos_since_epoch": 0},
    });
    let parsed: FsViolation = serde_json::from_value(wire.clone())?;
    assert_eq!(parsed.path.as_str(), "/etc/a/passwd");
    assert_eq!(parsed.operation, FsOperation::SetMetadata);
    assert_eq!(parsed.at, UNIX_EPOCH + Duration::from_secs(1));
    assert_eq!(serde_json::from_str::<FsViolation>(&wire.to_string())?, parsed);
    assert_eq!(
        serde_json::to_value(&parsed)?,
        json!({
            "lease_id": "lease-1",
            "path": "/etc/a/passwd",
            "operation": "set_metadata",
            "at": {"secs_since_epoch": 1, "nanos_since_epoch": 0},
        })
    );
    rejects::<FsViolation>(json!({
        "lease_id": "lease-1",
        "path": "relative",
        "operation": "read",
        "at": {"secs_since_epoch": 1, "nanos_since_epoch": 0},
    }))
}

#[test]
fn every_filesystem_operation_has_a_snake_case_name() -> TestResult {
    let cases = [
        (FsOperation::Read, "read"),
        (FsOperation::Write, "write"),
        (FsOperation::Create, "create"),
        (FsOperation::Delete, "delete"),
        (FsOperation::Rename, "rename"),
        (FsOperation::Link, "link"),
        (FsOperation::SetMetadata, "set_metadata"),
    ];
    for (operation, name) in cases {
        assert_eq!(serde_json::to_value(operation)?, json!(name));
        assert_eq!(serde_json::from_value::<FsOperation>(json!(name))?, operation);
    }
    rejects::<FsOperation>(json!("chmod"))
}

#[test]
fn a_literal_path_is_canonical_and_validated() -> TestResult {
    let ok = [
        ("/", "/"),
        ("/a/b/", "/a/b"),
        ("//a///b", "/a/b"),
        ("/a/./b", "/a/b"),
        ("/a/b/..", "/a"),
        ("/a/../b", "/b"),
    ];
    for (input, canonical) in ok {
        let path = LiteralPath::new(input)?;
        assert_eq!(path.as_str(), canonical);
        assert_eq!(path.to_string(), canonical);
        assert_eq!(LiteralPath::try_from(input.to_string())?, path);
        assert_eq!(input.parse::<LiteralPath>()?, path);
        assert_eq!(String::from(path.clone()), canonical);
        assert_eq!(serde_json::to_value(&path)?, json!(canonical));
        assert_eq!(serde_json::from_value::<LiteralPath>(json!(input))?, path);
    }
    let too_long = format!("/{}", "a".repeat(LiteralPath::MAX_LEN));
    let bad = [
        ("", LiteralPathError::Empty),
        ("a/b", LiteralPathError::NotAbsolute("a/b".to_string())),
        ("/a\0b", LiteralPathError::ContainsNul),
        (
            too_long.as_str(),
            LiteralPathError::TooLong {
                max: LiteralPath::MAX_LEN,
            },
        ),
        ("/..", LiteralPathError::EscapesRoot("/..".to_string())),
        ("/a/../..", LiteralPathError::EscapesRoot("/a/../..".to_string())),
    ];
    for (input, expected) in bad {
        assert_eq!(error_of(LiteralPath::new(input))?, expected);
        assert_eq!(error_of(input.parse::<LiteralPath>())?, expected);
        assert_eq!(error_of(LiteralPath::try_from(input.to_string()))?, expected);
        rejects::<LiteralPath>(json!(input))?;
    }
    Ok(())
}

#[test]
fn literal_path_errors_display_exactly() {
    let cases = [
        (LiteralPathError::Empty, "path cannot be empty"),
        (
            LiteralPathError::NotAbsolute("a".to_string()),
            "path \"a\" must be absolute",
        ),
        (LiteralPathError::ContainsNul, "path contains a NUL byte"),
        (
            LiteralPathError::TooLong { max: 4096 },
            "path is longer than 4096 bytes",
        ),
        (
            LiteralPathError::EscapesRoot("/..".to_string()),
            "path \"/..\" climbs above the root with \"..\"",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(error.to_string(), text);
    }
}

#[test]
fn a_secret_name_allows_a_small_alphabet() -> TestResult {
    for name in ["a", "API_KEY", "my.secret-1"] {
        let secret = SecretName::new(name)?;
        assert_eq!(secret.as_str(), name);
        assert_eq!(secret.to_string(), name);
        assert_eq!(SecretName::try_from(name.to_string())?, secret);
        assert_eq!(name.parse::<SecretName>()?, secret);
        assert_eq!(String::from(secret.clone()), name);
        assert_eq!(serde_json::to_value(&secret)?, json!(name));
        assert_eq!(serde_json::from_value::<SecretName>(json!(name))?, secret);
    }
    let bad = [
        ("", SecretNameError::Empty),
        ("a b", SecretNameError::InvalidCharacter),
        ("a/b", SecretNameError::InvalidCharacter),
        ("caf\u{e9}", SecretNameError::InvalidCharacter),
    ];
    for (input, expected) in bad {
        assert_eq!(error_of(SecretName::new(input))?, expected);
        assert_eq!(error_of(input.parse::<SecretName>())?, expected);
        assert_eq!(error_of(SecretName::try_from(input.to_string()))?, expected);
        rejects::<SecretName>(json!(input))?;
    }
    assert_eq!(SecretNameError::Empty.to_string(), "secret name must not be empty");
    assert_eq!(
        SecretNameError::InvalidCharacter.to_string(),
        "secret name must contain only ASCII letters, digits, '.', '_', or '-'"
    );
    Ok(())
}

#[test]
fn a_decide_request_is_tagged_by_its_method() -> TestResult {
    let bare = DecideRequest {
        lease_id: LeaseId::new("lease-1"),
        host: Host::new("example.com")?,
        port: Port::new(443)?,
        method: None,
    };
    let wire = json!({"lease_id": "lease-1", "host": "example.com", "port": 443});
    assert_eq!(serde_json::to_value(&bare)?, wire);
    assert_eq!(serde_json::from_value::<DecideRequest>(wire.clone())?, bare);
    assert_eq!(serde_json::from_str::<DecideRequest>(&wire.to_string())?, bare);

    let methods = [
        (DecideMethod::Connect, json!("connect")),
        (DecideMethod::Socks5, json!("socks5")),
        (DecideMethod::TlsSni, json!("tls_sni")),
        (DecideMethod::Http(HttpMethod::new("get")?), json!({"http": "GET"})),
    ];
    for (method, encoded) in methods {
        let request = DecideRequest {
            method: Some(method.clone()),
            ..bare.clone()
        };
        let wire = json!({"lease_id": "lease-1", "host": "example.com", "port": 443, "method": encoded});
        assert_eq!(serde_json::to_value(&request)?, wire);
        assert_eq!(serde_json::from_value::<DecideRequest>(wire)?, request);
    }
    Ok(())
}

#[test]
fn a_decide_request_rejects_invalid_parts() -> TestResult {
    rejects::<DecideRequest>(json!({"lease_id": "l", "host": "a b", "port": 443}))?;
    rejects::<DecideRequest>(json!({"lease_id": "l", "host": "example.com", "port": 0}))?;
    rejects::<DecideRequest>(json!({"lease_id": "l", "host": "example.com", "port": 1, "method": {"http": ""}}))?;
    rejects::<DecideRequest>(json!({"lease_id": "l", "host": "example.com", "port": 1, "method": "ftp"}))
}

#[test]
fn a_decide_response_is_tagged_by_its_verdict() -> TestResult {
    let cases = [
        (
            DecideResponse::Allow {
                scope: VerdictScope::Once,
            },
            json!({"verdict": "allow", "scope": "once"}),
        ),
        (
            DecideResponse::Allow {
                scope: VerdictScope::Persist,
            },
            json!({"verdict": "allow", "scope": "persist"}),
        ),
        (
            DecideResponse::Deny {
                scope: VerdictScope::Once,
            },
            json!({"verdict": "deny", "scope": "once"}),
        ),
        (
            DecideResponse::Deny {
                scope: VerdictScope::Persist,
            },
            json!({"verdict": "deny", "scope": "persist"}),
        ),
    ];
    for (response, wire) in cases {
        assert_eq!(serde_json::to_value(response)?, wire);
        assert_eq!(serde_json::from_value::<DecideResponse>(wire.clone())?, response);
        assert_eq!(serde_json::from_str::<DecideResponse>(&wire.to_string())?, response);
    }
    rejects::<DecideResponse>(json!({"verdict": "maybe", "scope": "once"}))?;
    rejects::<DecideResponse>(json!({"verdict": "allow", "scope": "forever"}))?;
    rejects::<DecideResponse>(json!({"verdict": "allow"}))?;
    rejects::<DecideResponse>(json!({"verdict": "allow", "scope": "once", "extra": 1}))?;
    assert_eq!(
        error_of(serde_json::from_str::<DecideResponse>("not json"))?.classify(),
        serde_json::error::Category::Syntax
    );
    Ok(())
}

#[test]
fn a_request_decide_request_omits_empty_parts() -> TestResult {
    let minimal = RequestDecideRequest {
        lease_id: LeaseId::new("lease-1"),
        method: "GET".to_string(),
        host: "example.com".to_string(),
        port: 443,
        host_header: None,
        sni: None,
        path: "/".to_string(),
        query: None,
        headers: BTreeMap::new(),
        body: None,
    };
    let wire = json!({"lease_id": "lease-1", "method": "GET", "host": "example.com", "port": 443, "path": "/"});
    assert_eq!(serde_json::to_value(&minimal)?, wire);
    assert_eq!(serde_json::from_value::<RequestDecideRequest>(wire)?, minimal);

    let full = RequestDecideRequest {
        host_header: Some("h.test".to_string()),
        sni: Some("s.test".to_string()),
        query: Some("a=1".to_string()),
        headers: BTreeMap::from([("accept".to_string(), vec!["a".to_string(), "b".to_string()])]),
        body: Some(RequestDecideBody {
            data: Base64Data::new("eHl6")?,
            cut: BodyTruncation::Truncated,
        }),
        ..minimal
    };
    let wire = json!({
        "lease_id": "lease-1",
        "method": "GET",
        "host": "example.com",
        "port": 443,
        "host_header": "h.test",
        "sni": "s.test",
        "path": "/",
        "query": "a=1",
        "headers": {"accept": ["a", "b"]},
        "body": {"data": "eHl6", "cut": true},
    });
    assert_eq!(serde_json::to_value(&full)?, wire);
    assert_eq!(serde_json::from_str::<RequestDecideRequest>(&wire.to_string())?, full);
    Ok(())
}

#[test]
fn a_body_marks_truncation_only_when_cut() -> TestResult {
    let complete = RequestDecideBody {
        data: Base64Data::new("YQ==")?,
        cut: BodyTruncation::Complete,
    };
    assert_eq!(serde_json::to_value(&complete)?, json!({"data": "YQ=="}));
    assert_eq!(
        serde_json::from_value::<RequestDecideBody>(json!({"data": "YQ=="}))?,
        complete
    );
    assert_eq!(
        serde_json::from_value::<RequestDecideBody>(json!({"data": "YQ==", "cut": false}))?,
        complete
    );
    assert_eq!(
        serde_json::from_value::<RequestDecideBody>(json!({"data": "YQ==", "cut": true}))?.cut,
        BodyTruncation::Truncated
    );
    assert!(BodyTruncation::default().is_complete());
    assert!(!BodyTruncation::Truncated.is_complete());
    assert_eq!(BodyTruncation::from(true), BodyTruncation::Truncated);
    assert!(bool::from(BodyTruncation::Truncated));
    assert!(!bool::from(BodyTruncation::Complete));
    rejects::<RequestDecideBody>(json!({"data": "YQ==", "extra": 1}))?;
    rejects::<RequestDecideBody>(json!({"data": "YQ==", "cut": "yes"}))
}

#[test]
fn a_request_decide_response_has_three_verdicts() -> TestResult {
    let allow_bare = RequestDecideResponse::Allow {
        set_headers: BTreeMap::new(),
        remove_headers: Vec::new(),
        cred: None,
        credential: None,
    };
    assert_eq!(serde_json::to_value(&allow_bare)?, json!({"verdict": "allow"}));
    assert_eq!(
        serde_json::from_value::<RequestDecideResponse>(json!({"verdict": "allow"}))?,
        allow_bare
    );

    let allow_full = RequestDecideResponse::Allow {
        set_headers: BTreeMap::from([("authorization".to_string(), vec!["Bearer s3cr3t".to_string()])]),
        remove_headers: vec!["cookie".to_string()],
        cred: Some("c1".to_string()),
        credential: Some("c2".to_string()),
    };
    let wire = json!({
        "verdict": "allow",
        "set_headers": {"authorization": ["Bearer s3cr3t"]},
        "remove_headers": ["cookie"],
        "cred": "c1",
        "credential": "c2",
    });
    assert_eq!(serde_json::to_value(&allow_full)?, wire);
    assert_eq!(
        serde_json::from_str::<RequestDecideResponse>(&wire.to_string())?,
        allow_full
    );

    let deny = RequestDecideResponse::Deny {
        status: 403,
        reason: Some("no".to_string()),
    };
    assert_eq!(
        serde_json::to_value(&deny)?,
        json!({"verdict": "deny", "status": 403, "reason": "no"})
    );
    let deny_bare = RequestDecideResponse::Deny {
        status: 403,
        reason: None,
    };
    assert_eq!(
        serde_json::to_value(&deny_bare)?,
        json!({"verdict": "deny", "status": 403})
    );
    assert_eq!(
        serde_json::from_value::<RequestDecideResponse>(json!({"verdict": "deny", "status": 403}))?,
        deny_bare
    );

    let need = RequestDecideResponse::NeedBody { max: 10 };
    assert_eq!(serde_json::to_value(&need)?, json!({"verdict": "need_body", "max": 10}));
    assert_eq!(
        serde_json::from_value::<RequestDecideResponse>(json!({"verdict": "need_body", "max": 10}))?,
        need
    );

    rejects::<RequestDecideResponse>(json!({"verdict": "allow", "other": 1}))?;
    rejects::<RequestDecideResponse>(json!({"verdict": "deny"}))?;
    rejects::<RequestDecideResponse>(json!({"verdict": "wait"}))
}

#[test]
fn a_request_decide_response_redacts_header_values_in_debug() {
    let allow = RequestDecideResponse::Allow {
        set_headers: BTreeMap::from([("authorization".to_string(), vec!["Bearer s3cr3t".to_string()])]),
        remove_headers: vec!["cookie".to_string()],
        cred: Some("c1".to_string()),
        credential: Some("hidden".to_string()),
    };
    assert_eq!(
        format!("{allow:?}"),
        "Allow { set_headers: [\"authorization\"], remove_headers: [\"cookie\"], cred: Some(\"c1\"), .. }"
    );
    let deny = RequestDecideResponse::Deny {
        status: 403,
        reason: Some("no".to_string()),
    };
    assert_eq!(format!("{deny:?}"), "Deny { status: 403, reason: Some(\"no\") }");
    assert_eq!(
        format!("{:?}", RequestDecideResponse::NeedBody { max: 7 }),
        "NeedBody { max: 7 }"
    );
}

#[test]
fn socket_marks_and_proxy_uids_have_fixed_values() -> TestResult {
    assert_eq!(SocketMark::EGRESS_PROXY.get(), 0x5352);
    assert_eq!(SocketMark::EGRESS_PROXY.to_string(), "0x5352");
    assert_eq!(ProxyUid::EGRESS_PROXY.get(), 65531);
    assert_eq!(ProxyUid::EGRESS_PROXY.to_string(), "65531");
    assert_eq!(ProxyUid::new(7), "7".parse::<ProxyUid>()?);
    assert!("-1".parse::<ProxyUid>().is_err());
    assert!("x".parse::<ProxyUid>().is_err());
    Ok(())
}
