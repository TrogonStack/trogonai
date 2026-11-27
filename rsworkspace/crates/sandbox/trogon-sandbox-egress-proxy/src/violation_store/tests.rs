use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use trogon_sandbox_contract::{EgressDecision, LeaseId, RefusedRequest};

fn ignoring(rules: &[(&str, &[&str])]) -> IgnoreViolations {
    IgnoreViolations::new(
        rules
            .iter()
            .map(|(key, patterns)| (key.to_string(), patterns.iter().map(|p| p.to_string()).collect()))
            .collect(),
    )
}

#[test]
fn ignores_nothing_without_rules() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!IgnoreViolations::default().ignores("deny file-read /x", Some("cat x")));
    Ok(())
}

#[test]
fn star_patterns_apply_to_every_command_and_to_none() -> Result<(), Box<dyn std::error::Error>> {
    let ignore = ignoring(&[("*", &["/tmp/noise"])]);
    assert!(ignore.ignores("deny file-read /tmp/noise/a", Some("cat a")));
    assert!(ignore.ignores("deny file-read /tmp/noise/a", None));
    assert!(!ignore.ignores("deny file-read /etc/shadow", Some("cat a")));
    Ok(())
}

#[test]
fn command_patterns_need_the_key_inside_the_command_text() -> Result<(), Box<dyn std::error::Error>> {
    let ignore = ignoring(&[("git push", &["ssh"])]);
    assert!(ignore.ignores("deny network ssh", Some("git push origin")));
    assert!(!ignore.ignores("deny network ssh", Some("git pull")));
    assert!(!ignore.ignores("deny network ssh", None));
    assert!(!ignore.ignores("deny network https", Some("git push origin")));
    Ok(())
}

#[test]
fn an_empty_pattern_matches_every_line() -> Result<(), Box<dyn std::error::Error>> {
    let ignore = ignoring(&[("*", &[""])]);
    assert!(ignore.ignores("anything", None));
    Ok(())
}

fn command(id: &str) -> Option<CommandId> {
    Some(CommandId::new(id))
}

#[test]
fn keeps_only_the_last_hundred_and_counts_all() -> Result<(), Box<dyn std::error::Error>> {
    let store = ViolationStore::new();
    for n in 0..150 {
        store.add(&format!("v{n}"), None);
    }
    assert_eq!(store.count(), MAX_RETAINED_VIOLATIONS);
    assert_eq!(store.total_count(), 150);
    assert_eq!(store.violations(None)[0].line.as_str(), "v50");
    let last = store.violations(Some(2));
    assert_eq!(last.len(), 2);
    assert_eq!(last[1].line.as_str(), "v149");
    Ok(())
}

#[test]
fn clear_forgets_violations_but_not_the_total() -> Result<(), Box<dyn std::error::Error>> {
    let store = ViolationStore::new();
    store.add("a", None);
    store.clear();
    assert_eq!(store.count(), 0);
    assert_eq!(store.total_count(), 1);
    Ok(())
}

#[test]
fn filters_by_command() -> Result<(), Box<dyn std::error::Error>> {
    let store = ViolationStore::new();
    store.add("a", command("one"));
    store.add("b", command("two"));
    store.add("c", command("one"));
    let one = store.violations_for_command(&CommandId::new("one"));
    let lines: Vec<_> = one.iter().map(|v| v.line.as_str()).collect();
    assert_eq!(lines, ["a", "c"]);
    Ok(())
}

#[test]
fn every_invisible_or_directional_character_becomes_a_space() -> Result<(), Box<dyn std::error::Error>> {
    let strip = [
        0x0000, 0x001b, 0x007f, 0x009b, 0x00ad, 0x061c, 0x200b, 0x200c, 0x200d, 0x200e, 0x200f, 0x2028, 0x2029, 0x202a,
        0x202b, 0x202c, 0x202d, 0x202e, 0x2060, 0x2061, 0x2062, 0x2063, 0x2064, 0x2066, 0x2067, 0x2068, 0x2069, 0xfeff,
        0xe0001, 0xe0041, 0xe007f,
    ];
    for code in strip {
        let character = char::from_u32(code).ok_or("expected a value")?;
        let text = format!("a{character}b");
        assert_eq!(ViolationLine::new(&text).as_str(), "a b", "U+{code:04X}");
        assert_eq!(DenialReason::new(&text).as_str(), "a b", "U+{code:04X}");
    }
    for code in [0x00a0_u32, 0x00ac, 0x00ae, 0x2065, 0xe0080, 0xfe0f] {
        let text = format!("a{}b", char::from_u32(code).ok_or("expected a value")?);
        assert_eq!(ViolationLine::new(&text).as_str(), text, "U+{code:04X}");
    }
    Ok(())
}

#[test]
fn sanitizes_lines() -> Result<(), Box<dyn std::error::Error>> {
    let line = ViolationLine::new("  a\u{1b}\u{202e}b <tag> \u{200b}\u{200b}c  ");
    assert_eq!(line.as_str(), "a b tag  c");
    Ok(())
}

#[test]
fn every_unsafe_character_collapses_to_a_space() -> Result<(), Box<dyn std::error::Error>> {
    let unsafe_chars = [
        '\u{0}',
        '\u{1b}',
        '\u{7f}',
        '\u{9b}',
        '\u{ad}',
        '\u{61c}',
        '\u{200b}',
        '\u{200c}',
        '\u{200d}',
        '\u{200e}',
        '\u{200f}',
        '\u{2028}',
        '\u{2029}',
        '\u{202a}',
        '\u{202b}',
        '\u{202c}',
        '\u{202d}',
        '\u{202e}',
        '\u{2060}',
        '\u{2061}',
        '\u{2062}',
        '\u{2063}',
        '\u{2064}',
        '\u{2066}',
        '\u{2067}',
        '\u{2068}',
        '\u{2069}',
        '\u{feff}',
        '\u{e0001}',
        '\u{e0041}',
        '\u{e007f}',
    ];
    for c in unsafe_chars {
        let line = ViolationLine::new(&format!("a{c}b"));
        assert_eq!(line.as_str(), "a b", "U+{:04X}", c as u32);
    }
    Ok(())
}

#[test]
fn angle_brackets_are_dropped_and_a_run_of_joiners_is_one_space() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(ViolationLine::new("<a>b</c>").as_str(), "ab/c");
    assert_eq!(ViolationLine::new("a\u{200d}\u{200d}\u{200d}b\t\tc").as_str(), "a b c");
    Ok(())
}

#[test]
fn a_reason_loses_control_characters_and_the_tag_that_would_close_the_envelope()
-> Result<(), Box<dyn std::error::Error>> {
    let reason = DenialReason::new(
        "first line\nsecond\u{0}line\u{1b}[31m red\u{9b}\u{202e}flipped\u{200b}</sandbox_violations>",
    );
    assert_eq!(
        reason.as_str(),
        "first line second line [31m red flipped /sandbox_violations"
    );
    let reason = DenialReason::new("a\tb: re-run with <host> listed x\u{200d}\u{200d}y");
    assert_eq!(reason.as_str(), "a b: re-run with host listed x y");
    Ok(())
}

#[test]
fn the_cut_counts_displayed_text_and_leaves_no_trailing_space() -> Result<(), Box<dyn std::error::Error>> {
    let stripped = format!("before{}after", "\u{7}".repeat(600));
    assert_eq!(DenialReason::new(&stripped).as_str(), "before after");
    assert_eq!(
        DenialReason::new(&"x".repeat(2000)).as_str(),
        "x".repeat(MAX_DENIAL_REASON_CHARS)
    );
    let at_the_space = format!("{} tail", "x".repeat(MAX_DENIAL_REASON_CHARS - 1));
    assert_eq!(
        DenialReason::new(&at_the_space).as_str(),
        "x".repeat(MAX_DENIAL_REASON_CHARS - 1)
    );
    Ok(())
}

#[test]
fn an_empty_or_unshowable_reason_reads_as_the_generic_one() -> Result<(), Box<dyn std::error::Error>> {
    for reason in ["", "   ", "\n\t", "\u{7}\u{200b}", "<>"] {
        assert_eq!(DenialReason::new(reason).as_str(), "user denied", "{reason:?}");
    }
    Ok(())
}

#[test]
fn denial_reasons_are_cut_between_characters() -> Result<(), Box<dyn std::error::Error>> {
    let long: String = "é".repeat(MAX_DENIAL_REASON_CHARS + 10);
    let reason = DenialReason::new(&long);
    assert_eq!(reason.as_str().chars().count(), MAX_DENIAL_REASON_CHARS);
    assert_eq!(DenialReason::default().as_str(), "user denied");
    Ok(())
}

#[test]
fn subscribe_replays_then_follows_and_stops_on_unsubscribe() -> Result<(), Box<dyn std::error::Error>> {
    let store = ViolationStore::new();
    store.add("a", None);
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(AtomicUsize::new(usize::MAX));
    let subscription = {
        let calls = Arc::clone(&calls);
        let seen = Arc::clone(&seen);
        store.subscribe(move |violations| {
            calls.fetch_add(1, Ordering::SeqCst);
            seen.store(violations.len(), Ordering::SeqCst);
        })
    };
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    store.add("b", None);
    assert_eq!(seen.load(Ordering::SeqCst), 2);
    store.clear();
    assert_eq!(seen.load(Ordering::SeqCst), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    subscription.unsubscribe();
    store.add("c", None);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    Ok(())
}

fn violation_line(host: Option<&str>, dest_ip: Option<std::net::IpAddr>, reason: EgressViolationReason) -> String {
    let sink = StoreViolationSink::new(ViolationStore::new(), CommandId::new("c"), DenialReasons::new());
    sink.describe(&EgressViolation {
        lease_id: LeaseId::new("lease-1"),
        host: host.map(str::to_string),
        port: 443,
        dest_ip,
        decision: EgressDecision::Deny,
        reason,
        binding: None,
        request: None,
        at: SystemTime::now(),
    })
}

#[test]
fn unlisted_hosts_read_as_not_on_the_allow_list() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        violation_line(Some("x.test"), None, EgressViolationReason::PolicyDenied),
        "deny network-outbound x.test:443 (host is not on the allow list)"
    );
    Ok(())
}

#[test]
fn address_classes_read_as_plain_words() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (AddressDenialClass::DenyListed, "a deny-listed address"),
        (AddressDenialClass::Loopback, "a loopback address"),
        (AddressDenialClass::Unspecified, "an unspecified address"),
        (AddressDenialClass::LinkLocal, "a link-local address"),
        (AddressDenialClass::Multicast, "a multicast address"),
        (AddressDenialClass::Broadcast, "the broadcast address"),
        (AddressDenialClass::CloudMetadata, "a cloud metadata address"),
        (AddressDenialClass::OwnInterface, "one of this host's addresses"),
        (AddressDenialClass::PrivateUse, "a listed address"),
        (AddressDenialClass::Listed, "a listed address"),
        (AddressDenialClass::NonLoopback, "a non-loopback address"),
    ];
    for (class, words) in cases {
        assert_eq!(
            violation_line(
                Some("x.test"),
                None,
                EgressViolationReason::DestinationAddressDenied(class)
            ),
            format!("deny network-outbound x.test:443 (resolved to {words})")
        );
    }
    Ok(())
}

#[test]
fn failed_permission_prompts_read_alike() -> Result<(), Box<dyn std::error::Error>> {
    for reason in [
        EgressViolationReason::DeciderTimedOut,
        EgressViolationReason::DeciderUnavailable,
        EgressViolationReason::DeciderMalformedReply,
        EgressViolationReason::DeciderOverloaded,
    ] {
        assert_eq!(
            violation_line(Some("x.test"), None, reason),
            "deny network-outbound x.test:443 (permission prompt failed)"
        );
    }
    Ok(())
}

#[test]
fn refused_opaque_tunnels_read_as_refused_connects() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        violation_line(Some("x.test"), None, EgressViolationReason::OpaqueTunnelRefused),
        "deny http-request CONNECT x.test:443 (this proxy does not tunnel CONNECTs it cannot inspect)"
    );
    Ok(())
}

#[test]
fn a_refused_socks_tunnel_reads_as_a_socks_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    let sink = StoreViolationSink::new(ViolationStore::new(), CommandId::new("c"), DenialReasons::new());
    let line = sink.describe(&EgressViolation {
        lease_id: LeaseId::new("lease-1"),
        host: Some("x.test".to_string()),
        port: 443,
        dest_ip: None,
        decision: EgressDecision::Deny,
        reason: EgressViolationReason::OpaqueTunnelRefused,
        binding: None,
        request: Some(RefusedRequest::new("SOCKS", "x.test:443", "refuseOpaqueTunnels")),
        at: SystemTime::now(),
    });
    assert_eq!(line, "deny socks-tunnel x.test:443 (refuseOpaqueTunnels)");
    Ok(())
}

#[test]
fn malformed_hosts_and_missing_policies_read_in_plain_words() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        violation_line(Some("bad host!"), None, EgressViolationReason::MalformedHost),
        "deny network-outbound bad host!:443 (malformed host)"
    );
    assert_eq!(
        violation_line(Some("x.test"), None, EgressViolationReason::PolicyUnavailable),
        "deny network-outbound x.test:443 (sandbox policy unavailable)"
    );
    Ok(())
}

#[test]
fn several_classes_are_joined_in_a_fixed_order() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        resolved_to_words(&[
            AddressDenialClass::CloudMetadata,
            AddressDenialClass::Loopback,
            AddressDenialClass::PrivateUse,
        ]),
        "resolved to a loopback address / a cloud metadata address / a listed address"
    );
    Ok(())
}

#[test]
fn a_tunnel_limit_refusal_reads_as_a_refused_connect() -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        violation_line(Some("x.test"), None, EgressViolationReason::TunnelLimitReached)
            .starts_with("deny http-request CONNECT x.test:443 (")
    );
    Ok(())
}

#[test]
fn a_refused_request_reads_as_an_http_request_with_method_and_target() -> Result<(), Box<dyn std::error::Error>> {
    let sink = StoreViolationSink::new(ViolationStore::new(), CommandId::new("c"), DenialReasons::new());
    let line = sink.describe(&EgressViolation {
        lease_id: LeaseId::new("lease-1"),
        host: Some("x.test".to_string()),
        port: 443,
        dest_ip: None,
        decision: EgressDecision::Deny,
        reason: EgressViolationReason::RequestDenied,
        binding: None,
        request: Some(RefusedRequest::new(
            "POST",
            "https://x.test/v1?key=secret",
            "blocked by policy",
        )),
        at: SystemTime::now(),
    });
    assert_eq!(
        line,
        "deny http-request POST https://x.test/v1?\u{2026} (blocked by policy)"
    );
    Ok(())
}

#[test]
fn identifiers_and_lines_display_as_their_text() -> Result<(), Box<dyn std::error::Error>> {
    let id = CommandId::new("cmd-7");
    assert_eq!(id.as_str(), "cmd-7");
    assert_eq!(format!("{id}"), "cmd-7");
    assert_eq!(format!("{}", ViolationLine::new("deny <x>")), "deny x");
    Ok(())
}

#[test]
fn a_default_store_starts_empty() -> Result<(), Box<dyn std::error::Error>> {
    let store = ViolationStore::default();
    assert_eq!(store.count(), 0);
    assert_eq!(store.total_count(), 0);
    Ok(())
}

#[test]
fn denial_reasons_keep_only_the_newest_entries() -> Result<(), Box<dyn std::error::Error>> {
    let reasons = DenialReasons::new();
    for index in 0..=MAX_REMEMBERED_REASONS {
        reasons.remember(&format!("h{index}.test"), 443, DenialReason::new("no"));
    }
    assert!(reasons.take("h0.test", 443).is_none());
    assert!(reasons.take("h1.test", 443).is_some());
    assert!(reasons.take(&format!("h{MAX_REMEMBERED_REASONS}.test"), 443).is_some());
    Ok(())
}

#[test]
fn ignore_rules_report_whether_any_exist() -> Result<(), Box<dyn std::error::Error>> {
    assert!(IgnoreViolations::default().is_empty());
    assert!(!ignoring(&[("*", &["x"])]).is_empty());
    Ok(())
}

#[test]
fn a_destination_without_a_host_is_named_by_address_or_left_unknown() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        violation_line(None, Some("203.0.113.9".parse()?), EgressViolationReason::MalformedHost),
        "deny network-outbound 203.0.113.9:443 (malformed host)"
    );
    assert_eq!(
        violation_line(None, None, EgressViolationReason::MalformedHost),
        "deny network-outbound unknown:443 (malformed host)"
    );
    Ok(())
}

#[test]
fn every_remaining_reason_reads_in_plain_words() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (EgressViolationReason::RequestDenied, "request refused by filter"),
        (
            EgressViolationReason::GssEncryptionUnsupported,
            "the proxy cannot carry GSS-encrypted PostgreSQL sessions",
        ),
        (
            EgressViolationReason::PostgresTlsDeclined,
            "the PostgreSQL server declined TLS, so a credential could not be substituted",
        ),
        (
            EgressViolationReason::SecretSentinelOffHost,
            "a masked credential was sent to a host it is not bound to",
        ),
        (
            EgressViolationReason::SecretCompressedResponse,
            "the response is compressed, so a credential could not be handled",
        ),
        (
            EgressViolationReason::SniDestinationMismatch,
            "the TLS server name does not match the requested host",
        ),
        (
            EgressViolationReason::SniResolutionFailed,
            "the TLS server name could not be resolved",
        ),
        (
            EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Streaming),
            "a masked AWS credential cannot be re-signed for a streaming-signed request",
        ),
        (
            EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Presigned),
            "a masked AWS credential cannot be re-signed for a presigned request",
        ),
        (
            EgressViolationReason::SecretSigv4Unsupported(UnsupportedSigv4Shape::Sigv4a),
            "a masked AWS credential cannot be re-signed for a SigV4A request",
        ),
        (
            EgressViolationReason::SecretSigv4Unsignable,
            "the request cannot be re-signed after substituting a masked AWS credential",
        ),
        (
            EgressViolationReason::SecretJwtMalformed,
            "a masked JWT credential is malformed",
        ),
        (
            EgressViolationReason::SecretJwtClaimsUnmatched,
            "the claims of a masked JWT credential do not match this request",
        ),
    ];
    for (reason, words) in cases {
        assert_eq!(
            violation_line(Some("x.test"), None, reason),
            format!("deny network-outbound x.test:443 ({words})")
        );
    }
    Ok(())
}

#[test]
fn every_address_class_reads_in_a_fixed_order() -> Result<(), Box<dyn std::error::Error>> {
    let classes = [
        AddressDenialClass::NonLoopback,
        AddressDenialClass::OwnInterface,
        AddressDenialClass::Listed,
        AddressDenialClass::PrivateUse,
        AddressDenialClass::CloudMetadata,
        AddressDenialClass::Broadcast,
        AddressDenialClass::Multicast,
        AddressDenialClass::LinkLocal,
        AddressDenialClass::Unspecified,
        AddressDenialClass::Loopback,
        AddressDenialClass::DenyListed,
    ];
    assert_eq!(
        resolved_to_words(&classes),
        "resolved to a deny-listed address / a loopback address / an unspecified address / a link-local address / a multicast address / the broadcast address / a cloud metadata address / a listed address / one of this host's addresses / a non-loopback address"
    );
    Ok(())
}

#[tokio::test]
async fn the_store_sink_records_what_it_is_given_and_refuses_what_it_cannot_read()
-> Result<(), Box<dyn std::error::Error>> {
    let store = ViolationStore::new();
    let sink = StoreViolationSink::new(store.clone(), CommandId::new("c"), DenialReasons::new());

    assert!(sink.publish("subject".to_string(), b"not json".to_vec()).await.is_err());
    assert_eq!(store.count(), 0);

    let payload = serde_json::to_vec(&EgressViolation {
        lease_id: LeaseId::new("lease-1"),
        host: Some("x.test".to_string()),
        port: 443,
        dest_ip: None,
        decision: EgressDecision::Deny,
        reason: EgressViolationReason::PolicyDenied,
        binding: None,
        request: None,
        at: SystemTime::now(),
    })?;
    sink.publish("subject".to_string(), payload.clone()).await?;
    assert_eq!(store.count(), 1);

    let ignoring_all = Arc::new(ignoring(&[("*", &["x.test"])]));
    let filtered = StoreViolationSink::new(store.clone(), CommandId::new("c"), DenialReasons::new())
        .with_filter(ViolationFilter::new(ignoring_all, None));
    filtered.publish("subject".to_string(), payload).await?;
    assert_eq!(store.count(), 1);
    Ok(())
}
