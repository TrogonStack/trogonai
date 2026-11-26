use super::*;

const ALL: [Fault; 9] = [
    Fault::AuthorityExpired,
    Fault::NoPermit,
    Fault::AddressForbidden,
    Fault::SniMismatch,
    Fault::PolicyUnavailable,
    Fault::AuditUnavailable,
    Fault::UpstreamUnreachable,
    Fault::MethodNotAllowed,
    Fault::BadRequest,
];

fn text(fault: Fault) -> String {
    String::from_utf8(fault.response()).expect("ascii response")
}

#[test]
fn statuses_follow_the_adr_table() {
    assert_eq!(Fault::NoPermit.status().code(), 403);
    assert_eq!(Fault::AddressForbidden.status().code(), 403);
    assert_eq!(Fault::SniMismatch.status().code(), 403);
    assert_eq!(Fault::PolicyUnavailable.status().code(), 503);
    assert_eq!(Fault::AuditUnavailable.status().code(), 503);
    assert_eq!(Fault::UpstreamUnreachable.status().code(), 502);
    assert_eq!(Fault::MethodNotAllowed.status().code(), 405);
    assert_eq!(Fault::BadRequest.status().code(), 400);
}

#[test]
fn every_response_carries_the_fault_header_and_a_body_naming_only_the_code() {
    for fault in ALL {
        let response = text(fault);
        let (head, body) = response.split_once("\r\n\r\n").expect("head and body");
        assert!(
            head.contains(&format!("{FAULT_HEADER}: {}", fault.code())),
            "{response}"
        );
        assert_eq!(body, format!("{}\n", fault.code()));
        assert!(head.contains(&format!("content-length: {}", body.len())));
        assert!(head.contains("connection: close"));
    }
}

#[test]
fn codes_are_distinct_screaming_snake_case() {
    let mut codes: Vec<&str> = ALL.iter().map(|fault| fault.code()).collect();
    for code in &codes {
        assert!(code.bytes().all(|b| b.is_ascii_uppercase() || b == b'_'), "{code}");
    }
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), ALL.len());
}

#[test]
fn only_503_carries_retry_after() {
    for fault in ALL {
        let has = text(fault).to_ascii_lowercase().contains("retry-after: ");
        assert_eq!(has, fault.status() == Status::ServiceUnavailable, "{fault}");
    }
    assert!(text(Fault::PolicyUnavailable).contains("retry-after: 5\r\n"));
    assert!(text(Fault::AuditUnavailable).contains("retry-after: 5\r\n"));
}

#[test]
fn an_audit_refusal_names_itself_in_the_header_and_the_body() {
    let response = text(Fault::AuditUnavailable);
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "{response}"
    );
    assert!(
        response.contains("x-trogon-egress-fault: AUDIT_UNAVAILABLE\r\n"),
        "{response}"
    );
    assert!(response.ends_with("\r\n\r\nAUDIT_UNAVAILABLE\n"), "{response}");
}

#[test]
fn every_audit_reason_code_is_the_response_code_in_lower_snake_case() {
    for fault in ALL {
        assert_eq!(fault.reason_code(), fault.code().to_ascii_lowercase(), "{fault}");
        assert!(trogon_audit::ReasonCode::new(fault.reason_code()).is_ok(), "{fault}");
    }
}

#[test]
fn only_405_carries_allow() {
    for fault in ALL {
        let has = text(fault).contains("allow: CONNECT\r\n");
        assert_eq!(has, fault == Fault::MethodNotAllowed, "{fault}");
    }
}

#[test]
fn status_line_is_well_formed() {
    assert!(text(Fault::NoPermit).starts_with("HTTP/1.1 403 Forbidden\r\n"));
    assert!(text(Fault::PolicyUnavailable).starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    assert!(text(Fault::UpstreamUnreachable).starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
    assert!(text(Fault::MethodNotAllowed).starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
}

#[test]
fn display_is_the_code() {
    for fault in ALL {
        assert_eq!(fault.to_string(), fault.code());
    }
}
