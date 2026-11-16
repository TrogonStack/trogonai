use std::time::{Duration, SystemTime};

use trogon_credential_domain::{CredentialId, CredentialKey};
use trogon_std::time::MockClock;

use super::ReleaseCache;
use crate::constants::{FULL_CACHE_WARNING_INTERVAL, RECENT_INVALIDATIONS};
use crate::fixtures::{CREDENTIAL_ID, VALUE, now, released_until, request, tenant};
use crate::{
    RELEASE_CACHE_JITTER, RELEASE_HOLD_BOUND, ReleaseCacheKey, ReleaseCacheMode, ReleaseHold, ReleaseInvalidation,
    ReleaseRequest, TenantAccount,
};

const OTHER_CREDENTIAL_ID: &str = "1c7e3a4f-8d2b-4a6c-9e01-2f3b4c5d6e7f";

fn key_of(tenant: &TenantAccount, credential_id: &str) -> ReleaseCacheKey {
    let asked = request(tenant);
    ReleaseCacheKey::of(&ReleaseRequest::new(
        tenant.clone(),
        CredentialKey::new(
            asked.credential().owner_id().clone(),
            CredentialId::new(credential_id).unwrap(),
        ),
        asked.destination().clone(),
        asked.location().clone(),
        asked.correlation_id().clone(),
    ))
}

fn in_a_minute() -> SystemTime {
    now() + Duration::from_secs(60)
}

fn in_an_hour() -> SystemTime {
    now() + Duration::from_secs(3_600)
}

/// A cache with room for one release, holding one under `key` that serves
/// for longer than any test moves its clock.
fn full_with(key: &ReleaseCacheKey, clock: MockClock) -> ReleaseCache<MockClock> {
    let mut cache = ReleaseCache::with_capacity(1, clock);
    cache.put(cache.ticket(), key.clone(), released_until(in_an_hour()), now());
    cache
}

fn notice_for(tenant: &TenantAccount, credential_id: &str) -> ReleaseInvalidation {
    ReleaseInvalidation::from_subject(&format!(
        "secrets.invalidation.v1.{tenant}.project~alpha.{credential_id}"
    ))
}

fn holding(key: &ReleaseCacheKey) -> ReleaseCache<MockClock> {
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    cache.put(cache.ticket(), key.clone(), released_until(in_a_minute()), now());
    cache
}

#[test]
fn holds_nothing_while_uncached() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    cache.suspend();

    cache.put(cache.ticket(), key.clone(), released_until(in_a_minute()), now());

    assert_eq!(cache.mode(), ReleaseCacheMode::Uncached);
    assert!(cache.get(&key, now()).is_none());
}

#[test]
fn does_not_hold_a_release_asked_for_before_an_invalidation_that_covers_it() {
    let tenant = tenant();
    let key = key_of(&tenant, CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    let ticket = cache.ticket();

    cache.invalidate(&notice_for(&tenant, OTHER_CREDENTIAL_ID));
    cache.invalidate(&notice_for(&tenant, CREDENTIAL_ID));

    assert!(!cache.is_current(ticket, &key));
    cache.put(ticket, key.clone(), released_until(in_a_minute()), now());
    assert!(cache.get(&key, now()).is_none());
}

#[test]
fn does_not_hold_a_release_asked_for_before_a_notice_it_could_not_read() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    let ticket = cache.ticket();

    cache.invalidate(&ReleaseInvalidation::from_subject("fake-subject"));

    assert!(!cache.is_current(ticket, &key));
}

#[test]
fn does_not_hold_a_release_asked_for_before_more_invalidations_than_it_remembers() {
    let tenant = tenant();
    let key = key_of(&tenant, CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    let ticket = cache.ticket();

    for _ in 0..=RECENT_INVALIDATIONS {
        cache.invalidate(&notice_for(&tenant, OTHER_CREDENTIAL_ID));
    }

    assert!(!cache.is_current(ticket, &key));
    cache.put(ticket, key.clone(), released_until(in_a_minute()), now());
    assert!(cache.get(&key, now()).is_none());
}

#[test]
fn holds_a_release_asked_for_before_as_many_unrelated_invalidations_as_it_remembers() {
    let tenant = tenant();
    let key = key_of(&tenant, CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    let ticket = cache.ticket();

    for _ in 0..RECENT_INVALIDATIONS {
        cache.invalidate(&notice_for(&tenant, OTHER_CREDENTIAL_ID));
    }

    assert!(cache.is_current(ticket, &key));
    cache.put(ticket, key.clone(), released_until(in_a_minute()), now());
    assert!(cache.get(&key, now()).is_some());
}

#[test]
fn does_not_hold_a_release_asked_for_before_the_reader_lost_its_place() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    let ticket = cache.ticket();

    cache.suspend();
    cache.resume();

    assert!(!cache.is_current(ticket, &key));
    cache.put(ticket, key.clone(), released_until(in_a_minute()), now());
    assert!(cache.get(&key, now()).is_none());
}

#[test]
fn counts_a_release_asked_for_while_uncached_as_stale_once_it_resumes() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    cache.suspend();
    let ticket = cache.ticket();

    cache.resume();

    assert!(!cache.is_current(ticket, &key));
}

#[test]
fn counts_a_release_asked_for_since_the_last_change_as_current() {
    let tenant = tenant();
    let key = key_of(&tenant, CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    cache.invalidate(&notice_for(&tenant, CREDENTIAL_ID));

    assert!(cache.is_current(cache.ticket(), &key));
}

#[test]
fn does_not_hold_a_release_whose_hold_is_over() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let released = released_until(in_a_minute());
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());

    cache.put(cache.ticket(), key.clone(), released.clone(), released.hold_until());
    cache.put(
        cache.ticket(),
        key.clone(),
        released,
        in_a_minute() + Duration::from_secs(1),
    );

    assert!(cache.get(&key, now()).is_none());
}

#[test]
fn does_not_hold_more_releases_than_it_has_room_for() {
    let tenant = tenant();
    let first = key_of(&tenant, CREDENTIAL_ID);
    let second = key_of(&tenant, OTHER_CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(1, MockClock::new());

    cache.put(cache.ticket(), first.clone(), released_until(in_a_minute()), now());
    cache.put(cache.ticket(), second.clone(), released_until(in_a_minute()), now());

    assert!(cache.get(&first, now()).is_some());
    assert!(cache.get(&second, now()).is_none());
}

#[test]
fn makes_room_by_dropping_copies_that_expired() {
    let tenant = tenant();
    let first = key_of(&tenant, CREDENTIAL_ID);
    let second = key_of(&tenant, OTHER_CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(1, MockClock::new());
    let later = in_a_minute() + Duration::from_secs(1);

    cache.put(cache.ticket(), first.clone(), released_until(in_a_minute()), now());
    cache.put(
        cache.ticket(),
        second.clone(),
        released_until(later + Duration::from_secs(60)),
        later,
    );

    assert!(cache.get(&second, later).is_some());
    assert!(cache.get(&first, now()).is_none());
}

#[test]
fn replaces_a_copy_it_holds_even_when_full() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(1, MockClock::new());
    let longer = now() + Duration::from_secs(120);

    cache.put(cache.ticket(), key.clone(), released_until(in_a_minute()), now());
    cache.put(cache.ticket(), key.clone(), released_until(longer), now());

    assert_eq!(cache.get(&key, now()).unwrap().valid_until(), longer);
}

#[test]
fn does_not_report_full_while_it_has_room() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(1, MockClock::new());

    let hold = cache.put(cache.ticket(), key, released_until(in_an_hour()), now());

    assert_eq!(hold, ReleaseHold::Held);
    assert_eq!(cache.capacity_warning(), None);
}

#[test]
fn does_not_report_full_when_it_replaces_a_copy_it_holds() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = full_with(&key, MockClock::new());

    let hold = cache.put(cache.ticket(), key, released_until(in_an_hour()), now());

    assert_eq!(hold, ReleaseHold::Held);
    assert_eq!(cache.capacity_warning(), None);
}

#[test]
fn does_not_report_full_when_it_declines_a_release_overtaken_on_its_way() {
    let tenant = tenant();
    let mut cache = full_with(&key_of(&tenant, CREDENTIAL_ID), MockClock::new());
    let ticket = cache.ticket();

    cache.invalidate(&notice_for(&tenant, OTHER_CREDENTIAL_ID));
    let hold = cache.put(
        ticket,
        key_of(&tenant, OTHER_CREDENTIAL_ID),
        released_until(in_an_hour()),
        now(),
    );

    assert_eq!(hold, ReleaseHold::Declined);
    assert_eq!(cache.capacity_warning(), None);
}

#[test]
fn does_not_report_full_once_it_made_room() {
    let tenant = tenant();
    let mut cache = ReleaseCache::with_capacity(1, MockClock::new());
    let later = in_a_minute() + Duration::from_secs(1);
    cache.put(
        cache.ticket(),
        key_of(&tenant, CREDENTIAL_ID),
        released_until(in_a_minute()),
        now(),
    );

    let hold = cache.put(
        cache.ticket(),
        key_of(&tenant, OTHER_CREDENTIAL_ID),
        released_until(later + Duration::from_secs(60)),
        later,
    );

    assert_eq!(hold, ReleaseHold::Held);
}

#[test]
fn reports_full_when_it_turns_a_release_away_for_room() {
    let tenant = tenant();
    let mut cache = full_with(&key_of(&tenant, CREDENTIAL_ID), MockClock::new());

    let hold = cache.put(
        cache.ticket(),
        key_of(&tenant, OTHER_CREDENTIAL_ID),
        released_until(in_an_hour()),
        now(),
    );

    assert_eq!(hold, ReleaseHold::Full);
}

#[test]
fn warns_of_a_full_cache_at_the_first_release_it_turns_away() {
    let tenant = tenant();
    let mut cache = full_with(&key_of(&tenant, CREDENTIAL_ID), MockClock::new());
    let turned_away = key_of(&tenant, OTHER_CREDENTIAL_ID);

    cache.put(cache.ticket(), turned_away, released_until(in_an_hour()), now());

    assert_eq!(cache.capacity_warning(), Some(1));
    assert_eq!(cache.capacity(), 1);
}

#[test]
fn does_not_warn_again_before_the_warning_interval_passes() {
    let tenant = tenant();
    let clock = MockClock::new();
    let mut cache = full_with(&key_of(&tenant, CREDENTIAL_ID), clock.clone());
    let turned_away = key_of(&tenant, OTHER_CREDENTIAL_ID);
    cache.put(cache.ticket(), turned_away.clone(), released_until(in_an_hour()), now());
    cache.capacity_warning();

    clock.advance(FULL_CACHE_WARNING_INTERVAL - Duration::from_secs(1));
    let hold = cache.put(cache.ticket(), turned_away, released_until(in_an_hour()), now());

    assert_eq!(hold, ReleaseHold::Full);
    assert_eq!(cache.capacity_warning(), None);
}

#[test]
fn warns_again_once_the_warning_interval_passes_with_every_release_turned_away_since() {
    let tenant = tenant();
    let clock = MockClock::new();
    let mut cache = full_with(&key_of(&tenant, CREDENTIAL_ID), clock.clone());
    let turned_away = key_of(&tenant, OTHER_CREDENTIAL_ID);
    cache.put(cache.ticket(), turned_away.clone(), released_until(in_an_hour()), now());
    cache.capacity_warning();
    cache.put(cache.ticket(), turned_away.clone(), released_until(in_an_hour()), now());
    cache.capacity_warning();

    clock.advance(FULL_CACHE_WARNING_INTERVAL);
    cache.put(cache.ticket(), turned_away, released_until(in_an_hour()), now());

    assert_eq!(cache.capacity_warning(), Some(2));
}

#[test]
fn stops_serving_a_copy_at_its_jittered_expiry() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = holding(&key);
    let expires_at = in_a_minute() - key.jitter_within(Duration::from_secs(60));

    assert!(cache.get(&key, expires_at - Duration::from_nanos(1)).is_some());
    assert!(cache.get(&key, expires_at).is_none());
    assert!(cache.get(&key, now()).is_none());
}

#[test]
fn never_serves_a_copy_past_its_hold() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    let released = released_until(now() + Duration::from_secs(3_600));

    cache.put(cache.ticket(), key.clone(), released, now());

    assert!(
        cache
            .get(&key, now() + RELEASE_HOLD_BOUND - RELEASE_CACHE_JITTER)
            .is_some()
    );
    assert!(cache.get(&key, now() + RELEASE_HOLD_BOUND).is_none());
}

#[test]
fn drops_every_copy_when_it_suspends() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = holding(&key);

    cache.suspend();

    assert!(cache.get(&key, now()).is_none());
}

#[test]
fn holds_again_once_it_resumes() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = ReleaseCache::with_capacity(8, MockClock::new());
    cache.suspend();
    cache.resume();

    cache.put(cache.ticket(), key.clone(), released_until(in_a_minute()), now());

    assert_eq!(cache.mode(), ReleaseCacheMode::Caching);
    assert!(cache.get(&key, now()).is_some());
}

#[test]
fn drops_only_the_copies_an_invalidation_covers() {
    let tenant = tenant();
    let named = key_of(&tenant, CREDENTIAL_ID);
    let other = key_of(&tenant, OTHER_CREDENTIAL_ID);
    let mut cache = holding(&named);
    cache.put(cache.ticket(), other.clone(), released_until(in_a_minute()), now());

    cache.invalidate(&notice_for(&tenant, CREDENTIAL_ID));

    assert!(cache.get(&named, now()).is_none());
    assert!(cache.get(&other, now()).is_some());
}

#[test]
fn serves_a_copy_it_holds() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let mut cache = holding(&key);

    let held = cache.get(&key, now()).unwrap();

    assert_eq!(held.value().as_str(), VALUE);
    assert_eq!(held.valid_until(), in_a_minute());
}

#[test]
fn serves_a_copy_until_its_monotonic_deadline_when_the_wall_clock_moves_back() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let clock = MockClock::new();
    let mut cache = ReleaseCache::with_capacity(8, clock.clone());
    cache.put(
        cache.ticket(),
        key.clone(),
        released_until(now() + Duration::from_secs(3_600)),
        now(),
    );

    clock.advance(RELEASE_HOLD_BOUND - RELEASE_CACHE_JITTER);

    assert!(cache.get(&key, now() - Duration::from_secs(3_600)).is_some());
}

#[test]
fn never_serves_a_copy_past_its_hold_when_the_wall_clock_moves_back() {
    let key = key_of(&tenant(), CREDENTIAL_ID);
    let clock = MockClock::new();
    let mut cache = ReleaseCache::with_capacity(8, clock.clone());
    cache.put(
        cache.ticket(),
        key.clone(),
        released_until(now() + Duration::from_secs(3_600)),
        now(),
    );

    clock.advance(RELEASE_HOLD_BOUND);

    assert!(cache.get(&key, now() - Duration::from_secs(3_600)).is_none());
}
