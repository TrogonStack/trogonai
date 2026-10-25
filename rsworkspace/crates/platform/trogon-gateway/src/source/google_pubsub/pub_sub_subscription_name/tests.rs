use super::*;

#[test]
fn subscription_name_accepts_canonical_form() {
    let name = PubSubSubscriptionName::new("projects/my-project/subscriptions/my-sub").unwrap();
    assert_eq!(name.as_str(), "projects/my-project/subscriptions/my-sub");
    assert_eq!(name.to_string(), "projects/my-project/subscriptions/my-sub");
}

#[test]
fn subscription_name_trims_surrounding_whitespace() {
    let name = PubSubSubscriptionName::new("  projects/p/subscriptions/sub \n").unwrap();
    assert_eq!(name.as_str(), "projects/p/subscriptions/sub");
}

#[test]
fn subscription_name_rejects_empty() {
    assert_eq!(
        PubSubSubscriptionName::new("   "),
        Err(PubSubSubscriptionNameError::Empty)
    );
}

#[test]
fn subscription_name_rejects_other_shapes() {
    for value in [
        "my-sub",
        "projects/p",
        "projects/p/topics/t",
        "projects//subscriptions/s",
        "projects/p/subscriptions/",
        "projects/p/subscriptions/s/extra",
        "Projects/p/subscriptions/s",
        "projects/p q/subscriptions/s",
    ] {
        assert_eq!(
            PubSubSubscriptionName::new(value),
            Err(PubSubSubscriptionNameError::InvalidShape),
            "{value}"
        );
    }
}

#[test]
fn subscription_name_accepts_every_allowed_id_character() {
    let name = PubSubSubscriptionName::new("projects/p/subscriptions/Sub-1_a.b~c+d%20").unwrap();
    assert_eq!(name.as_str(), "projects/p/subscriptions/Sub-1_a.b~c+d%20");
}

#[test]
fn subscription_name_rejects_ids_pubsub_would_refuse() {
    let too_long = format!("projects/p/subscriptions/s{}", "a".repeat(255));
    for value in [
        "projects/p/subscriptions/ab",
        "projects/p/subscriptions/1sub",
        "projects/p/subscriptions/-sub",
        "projects/p/subscriptions/sub?x",
        "projects/p/subscriptions/goog-sub",
        "projects/p/subscriptions/süb",
        too_long.as_str(),
    ] {
        assert_eq!(
            PubSubSubscriptionName::new(value),
            Err(PubSubSubscriptionNameError::InvalidSubscriptionId),
            "{value}"
        );
    }
}

#[test]
fn subscription_name_accepts_ids_at_the_length_bounds() {
    let longest = format!("projects/p/subscriptions/s{}", "a".repeat(254));
    for value in ["projects/p/subscriptions/abc", longest.as_str()] {
        assert!(PubSubSubscriptionName::new(value).is_ok(), "{value}");
    }
}
