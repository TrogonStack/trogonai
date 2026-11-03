#![allow(clippy::expect_used)]

use trogon_peer_nats::{
    PeerAccount, PeerCaller, PeerConnectionId, PeerIngress, PeerIngressError, PeerMethod, PeerPermissions,
    PeerPrincipal, PeerReplySubject, PeerRoute, PeerTenant,
};

fn principal(tenant: &str, caller: &str) -> PeerPrincipal {
    PeerPrincipal::new(
        PeerTenant::new(tenant).expect("valid tenant"),
        PeerCaller::new(caller).expect("valid caller"),
    )
}

fn shared() -> PeerIngress {
    PeerIngress::new(PeerAccount::SharedFixture {
        tenant: PeerTenant::new("tenant_a").expect("valid tenant"),
    })
}

#[test]
fn shared_subject_derives_the_principal_and_method() {
    let route = shared()
        .parse("tenant_a.peer.v1.caller_a.PeerService.Send")
        .expect("subject carries an authenticated routing principal");
    assert_eq!(route.principal(), &principal("tenant_a", "caller_a"));
    assert_eq!(route.method(), PeerMethod::Send);
}

#[test]
fn tenant_account_uses_its_configured_tenant() {
    let ingress = PeerIngress::new(PeerAccount::TenantAccount {
        tenant: PeerTenant::new("tenant_a").expect("valid tenant"),
    });
    let route = ingress
        .parse("peer.v1.caller_a.PeerService.PrepareBatch")
        .expect("tenant-account subjects do not carry a tenant token");
    assert_eq!(route.principal(), &principal("tenant_a", "caller_a"));
    assert_eq!(route.method(), PeerMethod::PrepareBatch);
    assert!(ingress.parse("tenant_b.peer.v1.caller_a.PeerService.Send").is_err());
}

#[test]
fn each_method_round_trips_without_request_ids_in_subjects() {
    for method in [
        PeerMethod::Enroll,
        PeerMethod::Heartbeat,
        PeerMethod::Detach,
        PeerMethod::Roster,
        PeerMethod::Send,
        PeerMethod::Status,
        PeerMethod::PrepareBatch,
        PeerMethod::AcknowledgeBatch,
    ] {
        let route = PeerRoute::new(principal("tenant_a", "caller_a"), method);
        let subject = shared().subject(&route).expect("valid route");
        assert_eq!(shared().parse(&subject).expect("generated subject"), route);
    }
}

#[test]
fn ingress_rejects_missing_principal_and_namespace_confusion() {
    for subject in [
        "tenant_a.peer.v1.PeerService.Send",
        "tenant_a.peer.v1..PeerService.Send",
        "tenant_a.peer.v1.*.PeerService.Send",
        "tenant_a.peer.v1.caller_a.PeerService.Send.extra",
        "tenant_a.peer.v10.caller_a.PeerService.Send",
        "tenant_a.peer_archive.v1.caller_a.PeerService.Send",
        "tenant_a.peer.v1.caller_a.OtherService.Send",
        "tenant_a.peer.v1.caller_a.PeerService.Unknown",
        "tenant_a.peer.v1.caller_a.PeerService.send",
        "tenant_a.peer.v1.caller_a.PeerService.>",
    ] {
        assert!(shared().parse(subject).is_err(), "accepted {subject:?}");
    }
}

#[test]
fn shared_fixture_cannot_be_re_scoped_by_a_subject_or_route() {
    assert_eq!(
        shared().parse("tenant_b.peer.v1.caller_a.PeerService.Send"),
        Err(PeerIngressError::TenantMismatch)
    );
    assert_eq!(
        shared().subject(&PeerRoute::new(principal("tenant_b", "caller_a"), PeerMethod::Send)),
        Err(PeerIngressError::TenantMismatch)
    );
}

#[test]
fn principal_tokens_exclude_controls_separators_and_wildcards() {
    for invalid in ["", "a.b", "a*", "a>", "a b", "a\0b", "a\u{7f}b", "a/b", "é"] {
        assert!(PeerTenant::new(invalid).is_err(), "accepted tenant {invalid:?}");
        assert!(PeerCaller::new(invalid).is_err(), "accepted caller {invalid:?}");
        assert!(
            PeerConnectionId::new(invalid).is_err(),
            "accepted connection {invalid:?}"
        );
    }
    assert!(PeerTenant::new("x".repeat(129)).is_err());
    assert!(PeerCaller::new("x".repeat(129)).is_err());
    assert!(PeerConnectionId::new("x".repeat(129)).is_err());
    assert!(PeerTenant::new("A".repeat(56)).is_ok());
    assert!(PeerCaller::new("caller-1_a").is_ok());
}

#[test]
fn reply_destination_is_confined_to_authenticated_tenant_and_caller() {
    let owner = principal("tenant_a", "caller_a");
    let reply = PeerReplySubject::parse_for(&owner, "_INBOX.tenant_a.caller_a.connection_1.nonce_1")
        .expect("requester owns the response destination");
    assert_eq!(reply.as_str(), "_INBOX.tenant_a.caller_a.connection_1.nonce_1");
    for invalid in [
        "_INBOX.tenant_b.caller_a.connection_1.nonce_1",
        "_INBOX.tenant_a.caller_b.connection_1.nonce_1",
        "_INBOX.tenant_a.caller_archive.connection_1.nonce_1",
        "_INBOX.tenant_a.caller_a.connection_1.*",
        "_INBOX.tenant_a.caller_a..nonce_1",
        "_INBOX.tenant_a.caller_a.connection_1.nonce_1.extra",
        "_INBOX.tenant_a.caller_a.connection_1",
        "_R_.broker_generated.import_reply",
        "tenant_a.peer.v1.caller_b.PeerService.Send",
    ] {
        assert!(
            PeerReplySubject::parse_for(&owner, invalid).is_err(),
            "accepted {invalid:?}"
        );
    }
}

#[test]
fn connector_grants_include_only_own_methods_and_unique_reply_subtree() {
    let owner = principal("tenant_a", "caller_a");
    let connection = PeerConnectionId::new("connection_1").expect("valid connection");
    let grants = PeerPermissions::for_connection(shared().account(), &owner, &connection).expect("scoped grants");
    assert_eq!(
        grants.publish_allow(),
        [
            "tenant_a.peer.v1.caller_a.PeerService.Enroll",
            "tenant_a.peer.v1.caller_a.PeerService.Heartbeat",
            "tenant_a.peer.v1.caller_a.PeerService.Detach",
            "tenant_a.peer.v1.caller_a.PeerService.Roster",
            "tenant_a.peer.v1.caller_a.PeerService.Send",
            "tenant_a.peer.v1.caller_a.PeerService.Status",
            "tenant_a.peer.v1.caller_a.PeerService.PrepareBatch",
            "tenant_a.peer.v1.caller_a.PeerService.AcknowledgeBatch",
        ]
    );
    assert_eq!(grants.subscribe_allow(), ["_INBOX.tenant_a.caller_a.connection_1.>"]);
    let successor = PeerConnectionId::new("connection_2").expect("different connection");
    let next = PeerPermissions::for_connection(shared().account(), &owner, &successor).expect("scoped grants");
    assert_ne!(grants.subscribe_allow(), next.subscribe_allow());
    assert_eq!(grants.publish_allow(), next.publish_allow());
}

#[test]
fn connector_grants_cannot_cross_configured_tenant() {
    let connection = PeerConnectionId::new("connection_1").expect("valid connection");
    assert_eq!(
        PeerPermissions::for_connection(shared().account(), &principal("tenant_b", "caller_a"), &connection),
        Err(PeerIngressError::TenantMismatch)
    );
}
