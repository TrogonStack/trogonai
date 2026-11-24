use super::*;

/// The same vectors as `trogon-audit`'s activation binding test, which repeats
/// this rule because that crate cannot depend on this one.
#[test]
fn accepts_and_refuses_the_same_ids_as_the_audit_contract() {
    let long_path = format!("spiffe://example.test/{}", "a".repeat(2048));
    for refused in [
        "spiffe://example.test",
        "spiffe://example.test/",
        "SPIFFE://example.test/proxy",
        "spiffe://EXAMPLE.test/proxy",
        "spiffe://example.test/proxy/",
        "spiffe://example.test//proxy",
        "spiffe://example.test/proxy/..",
        "spiffe://example.test/proxy?x=1",
        "spiffe://example.test/proxy#fragment",
        "spiffe://user@example.test/proxy",
        "spiffe://example.test:8080/proxy",
        "",
        long_path.as_str(),
    ] {
        assert!(WorkloadId::new(refused).is_err(), "{refused:?}");
    }
    for accepted in [
        "spiffe://example.test/proxy",
        "spiffe://example.test/opaque/deployment/7",
        "spiffe://a-b_c.example.test/A/b-c_d.e",
    ] {
        assert!(WorkloadId::new(accepted).is_ok(), "{accepted:?}");
    }
}

#[test]
fn a_lone_dot_segment_is_refused_wherever_it_appears() {
    for refused in [
        "spiffe://example.test/.",
        "spiffe://example.test/./proxy",
        "spiffe://example.test/proxy/.",
        "spiffe://example.test/proxy/./worker",
    ] {
        assert!(WorkloadId::new(refused).is_err(), "{refused:?}");
    }
    assert!(WorkloadId::new("spiffe://example.test/.proxy").is_ok());
    assert!(WorkloadId::new("spiffe://example.test/proxy.").is_ok());
}

fn trust_domain_of(bytes: usize) -> String {
    let label = "a".repeat(63);
    let mut name = String::new();
    while name.len() + label.len() + 1 < bytes {
        name.push_str(&label);
        name.push('.');
    }
    name.push_str(&"b".repeat(bytes - name.len()));
    assert_eq!(name.len(), bytes);
    name
}

#[test]
fn a_trust_domain_of_255_bytes_is_accepted_and_one_of_256_is_refused() {
    let accepted = format!("spiffe://{}/proxy", trust_domain_of(255));
    let refused = format!("spiffe://{}/proxy", trust_domain_of(256));
    assert!(WorkloadId::new(&accepted).is_ok());
    assert!(WorkloadId::new(&refused).is_err());
}
