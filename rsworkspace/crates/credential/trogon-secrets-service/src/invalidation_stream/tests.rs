use std::time::Duration;

use async_nats::jetstream::stream::{DiscardPolicy, PersistenceMode, RetentionPolicy, StorageType};

use super::*;

fn one() -> NonZeroUsize {
    NonZeroUsize::MIN
}

fn three() -> NonZeroUsize {
    NonZeroUsize::new(3).unwrap()
}

#[test]
fn refuses_each_way_a_stream_could_weaken_the_guarantees() {
    type Weaken = fn(&mut Config);
    let cases: [(Weaken, InvalidationStreamMismatchError); 18] = [
        (
            |c| c.storage = StorageType::Memory,
            InvalidationStreamMismatchError::Storage,
        ),
        (
            |c| c.retention = RetentionPolicy::Interest,
            InvalidationStreamMismatchError::Retention,
        ),
        (
            |c| c.retention = RetentionPolicy::WorkQueue,
            InvalidationStreamMismatchError::Retention,
        ),
        (|c| c.deny_delete = false, InvalidationStreamMismatchError::DenyDelete),
        (|c| c.deny_purge = false, InvalidationStreamMismatchError::DenyPurge),
        (|c| c.allow_rollup = true, InvalidationStreamMismatchError::Rollup),
        (|c| c.no_ack = true, InvalidationStreamMismatchError::NoAck),
        (|c| c.sealed = true, InvalidationStreamMismatchError::Sealed),
        (
            |c| c.persist_mode = Some(PersistenceMode::Async),
            InvalidationStreamMismatchError::AsyncPersist,
        ),
        (
            |c| c.max_age = Duration::from_secs(329),
            InvalidationStreamMismatchError::MaxAge,
        ),
        (
            |c| c.max_age = Duration::from_secs(60),
            InvalidationStreamMismatchError::MaxAge,
        ),
        (
            |c| c.allow_message_ttl = true,
            InvalidationStreamMismatchError::MessageTtl,
        ),
        (
            |c| c.max_messages_per_subject = 1,
            InvalidationStreamMismatchError::MaxMessagesPerSubject,
        ),
        (
            |c| c.max_message_size = 1023,
            InvalidationStreamMismatchError::MaxMessageSize,
        ),
        (
            |c| c.max_message_size = 1,
            InvalidationStreamMismatchError::MaxMessageSize,
        ),
        (
            |c| c.duplicate_window = Duration::from_secs(119),
            InvalidationStreamMismatchError::DuplicateWindow,
        ),
        (|c| c.num_replicas = 2, InvalidationStreamMismatchError::Replicas),
        (|c| c.subjects.clear(), InvalidationStreamMismatchError::MissingSubject),
    ];
    for (weaken, expected) in cases {
        let mut actual = InvalidationStream::config(three());
        weaken(&mut actual);
        assert_eq!(
            InvalidationStream::validate(three(), &actual),
            Err(expected),
            "{expected:?} was not refused"
        );
    }
}

#[test]
fn refuses_a_stream_whose_only_subject_is_another_one() {
    let mut actual = InvalidationStream::config(one());
    actual.subjects = vec!["*.secrets.invalidation.v2.>".to_owned()];

    assert_eq!(
        InvalidationStream::validate(one(), &actual),
        Err(InvalidationStreamMismatchError::MissingSubject)
    );
}

#[test]
fn leaves_capacity_placement_and_stronger_settings_alone() {
    type Change = fn(&mut Config);
    let cases: [Change; 14] = [
        |c| c.discard = DiscardPolicy::New,
        |c| c.max_bytes = 1 << 20,
        |c| c.max_messages = 10_000,
        |c| c.allow_direct = false,
        |c| c.subjects.push("extra.subject".to_owned()),
        |c| c.persist_mode = Some(PersistenceMode::Default),
        |c| c.max_age = Duration::ZERO,
        |c| c.max_age = Duration::from_secs(330),
        |c| c.max_age = Duration::from_secs(86_400),
        |c| c.max_message_size = -1,
        |c| c.max_message_size = 1 << 20,
        |c| c.max_messages_per_subject = -1,
        |c| c.duplicate_window = Duration::from_secs(600),
        |c| c.num_replicas = 5,
    ];
    for change in cases {
        let mut actual = InvalidationStream::config(three());
        change(&mut actual);
        assert_eq!(InvalidationStream::validate(three(), &actual), Ok(()), "{actual:?}");
    }
}

#[test]
fn the_configuration_it_creates_is_one_it_accepts() {
    for replicas in [one(), three()] {
        assert_eq!(
            InvalidationStream::validate(replicas, &InvalidationStream::config(replicas)),
            Ok(())
        );
    }
}

#[test]
fn config_holds_every_guarantee_the_contract_names() {
    for replicas in [one(), three()] {
        let config = InvalidationStream::config(replicas);

        assert_eq!(config.name, "SECRETS_INVALIDATION");
        assert_eq!(config.subjects, ["*.secrets.invalidation.v1.>"]);
        assert_eq!(config.storage, StorageType::File);
        assert_eq!(config.retention, RetentionPolicy::Limits);
        assert_eq!(config.discard, DiscardPolicy::Old);
        assert_eq!(config.max_age, Duration::from_secs(3600));
        assert_eq!(config.duplicate_window, Duration::from_secs(120));
        assert_eq!(config.max_message_size, 1024);
        assert!(config.deny_delete);
        assert!(config.deny_purge);
        assert!(config.allow_direct);
        assert!(!config.allow_rollup);
        assert!(!config.allow_message_ttl);
        assert!(!config.no_ack);
        assert!(!config.sealed);
        assert_eq!(config.persist_mode, None);
        assert_eq!(config.max_messages, 0);
        assert_eq!(config.max_bytes, 0);
        assert_eq!(config.max_messages_per_subject, 0);
        assert_eq!(config.num_replicas, replicas.get());
    }
}
