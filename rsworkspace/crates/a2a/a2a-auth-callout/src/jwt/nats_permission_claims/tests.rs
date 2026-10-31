use super::*;
use crate::permissions::SubjectPattern;

fn pattern(raw: &str) -> SubjectPattern {
    SubjectPattern::new(raw).expect("subject pattern")
}

#[test]
fn allow_only_keeps_the_listed_subjects_and_relies_on_nats_to_refuse_the_rest() {
    let permission = NatsSubjectPermission::allow_only(["a2a.v1.gateway.>".to_owned()]);

    assert_eq!(permission.allow, vec!["a2a.v1.gateway.>"]);
    assert!(permission.deny.is_empty());
}

#[test]
fn allow_only_with_nothing_listed_denies_every_subject() {
    let permission = NatsSubjectPermission::allow_only(std::iter::empty());

    assert!(permission.allow.is_empty());
    assert_eq!(permission.deny, vec![">"]);
}

#[test]
fn issued_permissions_without_subscribe_grants_mint_a_user_that_cannot_subscribe() {
    let issued = IssuedPermissions {
        publish_allow: vec![pattern("a2a.v1.gateway.>")],
        subscribe_allow: Vec::new(),
    };

    let claims = NatsPermissionClaims::from(&issued);

    assert_eq!(claims.publish.allow, vec!["a2a.v1.gateway.>"]);
    assert!(claims.publish.deny.is_empty());
    assert!(claims.subscribe.allow.is_empty());
    assert_eq!(claims.subscribe.deny, vec![">"]);
}

#[test]
fn issued_permissions_without_any_grant_mint_a_user_that_can_do_nothing() {
    let issued = IssuedPermissions {
        publish_allow: Vec::new(),
        subscribe_allow: Vec::new(),
    };

    let claims = NatsPermissionClaims::from(&issued);

    assert_eq!(claims.publish.deny, vec![">"]);
    assert_eq!(claims.subscribe.deny, vec![">"]);
}
