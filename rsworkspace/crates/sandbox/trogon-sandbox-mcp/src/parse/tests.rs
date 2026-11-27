use std::collections::BTreeMap;

use trogon_sandbox_contract::{GroupMembershipAction, LeaseStatus, SuspendRequestLevel, UnixSignal};

use super::{
    digest, execution_id, group_name, lease_id, lease_name, membership_action, optional_username, signal,
    snapshot_tag_name, status_filter, suspend_level, tags, username,
};

#[test]
fn a_lease_id_keeps_its_text() {
    assert_eq!(lease_id("l-1".to_string()).as_str(), "l-1");
}

#[test]
fn tags_are_bounded() {
    let many: BTreeMap<String, String> = (0..6).map(|i| (format!("k{i}"), "v".to_string())).collect();
    assert!(tags(many).is_err());
    assert!(tags(BTreeMap::new()).is_ok());
}

#[test]
fn a_status_filter_is_optional_and_typed() {
    assert!(matches!(status_filter(None), Ok(None)));
    assert!(matches!(
        status_filter(Some("running".to_string())),
        Ok(Some(LeaseStatus::Running))
    ));
    assert!(status_filter(Some("nonsense".to_string())).is_err());
}

#[test]
fn a_suspend_level_defaults_to_auto_and_rejects_unknown_names() {
    assert!(matches!(suspend_level(None), Ok(SuspendRequestLevel::Auto)));
    assert!(matches!(
        suspend_level(Some("auto".to_string())),
        Ok(SuspendRequestLevel::Auto)
    ));
    assert!(matches!(
        suspend_level(Some("memory".to_string())),
        Ok(SuspendRequestLevel::Memory)
    ));
    assert!(matches!(
        suspend_level(Some("disk".to_string())),
        Ok(SuspendRequestLevel::Disk)
    ));
    assert!(suspend_level(Some("warm".to_string())).is_err());
}

#[test]
fn an_execution_id_keeps_its_text() {
    assert_eq!(execution_id("e-1".to_string()).as_str(), "e-1");
}

#[test]
fn a_signal_defaults_to_term_and_ignores_case() {
    assert!(matches!(signal(None), Ok(UnixSignal::Term)));
    let named = [
        ("term", UnixSignal::Term),
        ("KILL", UnixSignal::Kill),
        ("int", UnixSignal::Int),
        ("hup", UnixSignal::Hup),
        ("quit", UnixSignal::Quit),
        ("usr1", UnixSignal::Usr1),
        ("usr2", UnixSignal::Usr2),
    ];
    for (name, expected) in named {
        assert!(
            matches!(signal(Some(name.to_string())), Ok(s) if s == expected),
            "{name}"
        );
    }
    assert!(signal(Some("stop".to_string())).is_err());
}

#[test]
fn a_username_is_optional_and_typed() {
    assert!(matches!(optional_username(None), Ok(None)));
    assert!(matches!(optional_username(Some("dev".to_string())), Ok(Some(_))));
    assert!(optional_username(Some(String::new())).is_err());
}

#[test]
fn account_names_are_validated() {
    assert!(username("dev".to_string()).is_ok());
    assert!(username("Not Valid!".to_string()).is_err());
    assert!(group_name("team".to_string()).is_ok());
    assert!(group_name(String::new()).is_err());
}

#[test]
fn a_membership_action_is_add_or_remove() {
    assert!(matches!(
        membership_action("add".to_string()),
        Ok(GroupMembershipAction::Add)
    ));
    assert!(matches!(
        membership_action("remove".to_string()),
        Ok(GroupMembershipAction::Remove)
    ));
    assert!(membership_action("grant".to_string()).is_err());
}

#[test]
fn lease_and_snapshot_names_are_validated() {
    assert!(lease_name("my-lease".to_string()).is_ok());
    assert!(lease_name(String::new()).is_err());
    assert!(snapshot_tag_name("stable".to_string()).is_ok());
    assert!(snapshot_tag_name(String::new()).is_err());
    assert!(digest("not a digest".to_string()).is_err());
}
