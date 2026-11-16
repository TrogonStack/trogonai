use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;
use tokio::time::Instant;

use trogon_credential_domain::{CredentialId, CredentialKey};
use trogon_semconv::metric::{
    SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES, SECRETS_CLIENT_RELEASE_CACHE_CAPACITY_SKIPS,
};
use trogon_std::SecretString;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

use super::CachedReleaseClient;
use crate::constants::FULL_CACHE_WARNING_INTERVAL;
use crate::fixtures::{
    BUNDLE_GENERATION, CREDENTIAL_ID, VALUE, invalidation_stream_config, now, request, runtime_service, secret, tenant,
};
use crate::telemetry::RecordedMetrics;
use crate::{
    FakeOutcome, FakeSecretsService, INVALIDATION_POSITION_CHECK, INVALIDATION_RETRY_DELAY, INVALIDATION_STREAM,
    InvalidationStartError, InvalidationStep, InvalidationStreamError, ReleaseCacheMode, ReleaseClient,
    ReleaseClientConfig, ReleaseError, ReleaseRequest, ReleasedValue, ScriptedInvalidations, SteppedClock,
    TenantAccount,
};

const OTHER_CREDENTIAL_ID: &str = "1c7e3a4f-8d2b-4a6c-9e01-2f3b4c5d6e7f";
const REVOKED: &str = "fake-revoked-secret";
const REPLY_DELAY: Duration = Duration::from_secs(1);
const LOST_PLACE: &str = "invalidation reader lost its place; releases are not held until it reads again";
const READS_AGAIN: &str = "invalidation reader reads again; releases are held again";
const CACHE_FULL: &str = "release cache is full; releases are served without being held";

type Client = CachedReleaseClient<FakeSecretsService, SteppedClock>;

fn in_a_minute() -> SystemTime {
    now() + Duration::from_secs(60)
}

fn releasing(fake: &FakeSecretsService, times: usize) {
    for _ in 0..times {
        fake.script(FakeOutcome::release(secret(), in_a_minute(), BUNDLE_GENERATION));
    }
}

/// Releases `value` only after [`REPLY_DELAY`], so a test can deliver a
/// notice while the release is on its way.
fn releasing_late(fake: &FakeSecretsService, value: &str) {
    fake.script(
        FakeOutcome::release(SecretString::new(value).unwrap(), in_a_minute(), BUNDLE_GENERATION).after(REPLY_DELAY),
    );
}

/// Starts `asked` in the background and lets it reach the service.
async fn releasing_in_background(
    client: &Client,
    asked: &ReleaseRequest,
) -> JoinHandle<Result<ReleasedValue, ReleaseError>> {
    let (client, asked) = (client.clone(), asked.clone());
    let released = tokio::spawn(async move { client.release(&asked).await });
    settle().await;
    released
}

/// Lets the reader reach its next look at how far its consumer delivered.
async fn next_position_check() {
    tokio::time::sleep(INVALIDATION_POSITION_CHECK).await;
}

/// Lets a late reply in flight arrive.
async fn reply() {
    tokio::time::sleep(REPLY_DELAY).await;
}

fn notice_subject(tenant: &TenantAccount, credential_id: &str) -> String {
    format!("secrets.invalidation.v1.{tenant}.project~alpha.{credential_id}")
}

fn of_credential(asked: &ReleaseRequest, credential_id: &str) -> ReleaseRequest {
    ReleaseRequest::new(
        asked.tenant().clone(),
        CredentialKey::new(
            asked.credential().owner_id().clone(),
            CredentialId::new(credential_id).unwrap(),
        ),
        asked.destination().clone(),
        asked.location().clone(),
        asked.correlation_id().clone(),
    )
}

async fn started(
    fake: &FakeSecretsService,
    invalidations: &ScriptedInvalidations,
) -> Result<Client, InvalidationStartError> {
    started_on(fake, invalidations, SteppedClock::at(now())).await
}

async fn started_on(
    fake: &FakeSecretsService,
    invalidations: &ScriptedInvalidations,
    clock: SteppedClock,
) -> Result<Client, InvalidationStartError> {
    let client = ReleaseClient::new(
        fake.clone(),
        clock,
        ReleaseClientConfig::new(runtime_service(), fake.service_xkey()),
    );
    CachedReleaseClient::start(client, invalidations.clone()).await
}

/// Starts a client that holds at most `capacity` releases and records on
/// `recorded`.
async fn started_recording(
    fake: &FakeSecretsService,
    invalidations: &ScriptedInvalidations,
    clock: SteppedClock,
    capacity: usize,
    recorded: &RecordedMetrics,
) -> Client {
    let client = ReleaseClient::new(
        fake.clone(),
        clock,
        ReleaseClientConfig::new(runtime_service(), fake.service_xkey()),
    );
    CachedReleaseClient::start_with(client, invalidations.clone(), capacity, &recorded.meter())
        .await
        .unwrap()
}

/// Releases that serve for longer than any test moves its clock.
fn releasing_for_an_hour(fake: &FakeSecretsService, times: usize) {
    for _ in 0..times {
        fake.script(FakeOutcome::release(
            secret(),
            now() + Duration::from_secs(3_600),
            BUNDLE_GENERATION,
        ));
    }
}

/// The reason and detail of every loss of place the reader warned of.
fn losses_warned(events: &CapturedEvents) -> Vec<(String, String)> {
    events
        .events()
        .iter()
        .filter(|event| event.message() == Some(LOST_PLACE))
        .map(|event| {
            (
                event.field("reason").unwrap().to_owned(),
                event.field("detail").unwrap().to_owned(),
            )
        })
        .collect()
}

fn reasons_warned(events: &CapturedEvents) -> Vec<String> {
    losses_warned(events).into_iter().map(|(reason, _)| reason).collect()
}

fn losses_counted(reason: &str) -> Vec<(Vec<String>, u64)> {
    vec![(vec![format!("secrets.client.invalidation.loss_reason={reason}")], 1)]
}

/// Lets the reader act on everything sent to it, without reaching its retry.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

/// Lets the reader reach its next retry.
async fn retry() {
    tokio::time::sleep(INVALIDATION_RETRY_DELAY + Duration::from_millis(1)).await;
}

#[tokio::test(start_paused = true)]
async fn refuses_to_start_without_the_invalidation_stream() {
    let invalidations = ScriptedInvalidations::default();
    invalidations.refuse(InvalidationStep::Stream);

    let refused = started(&FakeSecretsService::new(), &invalidations).await.err().unwrap();

    assert!(matches!(refused, InvalidationStartError::Stream(_)));
    assert_eq!(refused.to_string(), "the invalidation stream could not be found");
    assert_eq!(invalidations.streams(), [INVALIDATION_STREAM]);
}

#[tokio::test(start_paused = true)]
async fn refuses_to_start_on_a_stream_that_could_drop_a_notice_inside_the_lookback() {
    let invalidations = ScriptedInvalidations::default();
    let mut config = invalidation_stream_config();
    config.deny_purge = false;
    invalidations.configure_stream(config);

    let refused = started(&FakeSecretsService::new(), &invalidations).await.err().unwrap();

    assert!(matches!(
        refused,
        InvalidationStartError::StreamConfig(InvalidationStreamError::PurgesAllowed)
    ));
    assert_eq!(
        refused.to_string(),
        "the invalidation stream lets its notices be purged"
    );
    assert!(invalidations.configs().is_empty());
}

#[tokio::test(start_paused = true)]
async fn keeps_holding_nothing_while_the_stream_could_drop_a_notice() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    let mut config = invalidation_stream_config();
    config.max_age = Duration::from_secs(60);
    invalidations.configure_stream(config);

    invalidations.fail();
    retry().await;
    retry().await;
    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
    invalidations.configure_stream(invalidation_stream_config());
    retry().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    assert_eq!(invalidations.configs().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn refuses_to_start_when_it_cannot_create_its_consumer() {
    let invalidations = ScriptedInvalidations::default();
    invalidations.refuse(InvalidationStep::Consumer);

    let refused = started(&FakeSecretsService::new(), &invalidations).await.err().unwrap();

    assert!(matches!(refused, InvalidationStartError::Consumer(_)));
    assert_eq!(refused.to_string(), "the invalidation consumer could not be created");
}

#[tokio::test(start_paused = true)]
async fn refuses_to_start_when_it_cannot_read_its_consumer() {
    let invalidations = ScriptedInvalidations::default();
    invalidations.refuse(InvalidationStep::Messages);

    let refused = started(&FakeSecretsService::new(), &invalidations).await.err().unwrap();

    assert!(matches!(refused, InvalidationStartError::Messages(_)));
    assert_eq!(refused.to_string(), "the invalidation consumer could not be read");
}

#[tokio::test(start_paused = true)]
async fn does_not_hold_a_failed_release() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::unavailable());
    releasing(&fake, 1);
    let client = started(&fake, &ScriptedInvalidations::default()).await.unwrap();
    let asked = request(&tenant());

    assert!(matches!(
        client.release(&asked).await.unwrap_err(),
        ReleaseError::Unavailable
    ));
    assert_eq!(client.release(&asked).await.unwrap().value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn asks_again_once_an_invalidation_names_the_credential() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 2);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());
    client.release(&asked).await.unwrap();

    invalidations.notify(&notice_subject(asked.tenant(), CREDENTIAL_ID));
    settle().await;
    client.release(&asked).await.unwrap();

    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn asks_again_for_everything_after_a_notice_it_cannot_read() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 2);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());
    client.release(&asked).await.unwrap();

    invalidations.notify("fake-subject");
    settle().await;
    client.release(&asked).await.unwrap();

    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn keeps_serving_other_credentials_after_an_invalidation() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 2);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let named = request(&tenant());
    let other = of_credential(&named, OTHER_CREDENTIAL_ID);
    client.release(&named).await.unwrap();
    client.release(&other).await.unwrap();

    invalidations.notify(&notice_subject(named.tenant(), CREDENTIAL_ID));
    settle().await;
    client.release(&other).await.unwrap();

    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn serves_uncached_while_its_reader_has_lost_its_place() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 3);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());
    client.release(&asked).await.unwrap();

    invalidations.fail();
    settle().await;
    client.release(&asked).await.unwrap();
    client.release(&asked).await.unwrap();

    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
    assert_eq!(fake.received().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn serves_uncached_once_its_feed_ends() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 2);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());
    client.release(&asked).await.unwrap();

    invalidations.end();
    settle().await;
    client.release(&asked).await.unwrap();

    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn serves_uncached_once_a_notice_goes_missing() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 2);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());
    client.release(&asked).await.unwrap();

    invalidations.skip_one(&notice_subject(asked.tenant(), OTHER_CREDENTIAL_ID));
    settle().await;
    client.release(&asked).await.unwrap();

    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn serves_uncached_once_a_notice_cannot_be_placed() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 2);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());
    client.release(&asked).await.unwrap();

    invalidations.notify_unplaced(&notice_subject(asked.tenant(), OTHER_CREDENTIAL_ID));
    settle().await;
    client.release(&asked).await.unwrap();

    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn keeps_its_place_while_notices_arrive_in_order() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    let subject = notice_subject(&tenant(), OTHER_CREDENTIAL_ID);

    invalidations.notify(&subject);
    invalidations.notify(&subject);
    settle().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
}

#[tokio::test(start_paused = true)]
async fn counts_a_new_consumer_from_its_first_notice() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    let subject = notice_subject(&tenant(), OTHER_CREDENTIAL_ID);
    invalidations.notify(&subject);
    invalidations.fail();
    retry().await;

    invalidations.notify(&subject);
    settle().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
}

#[tokio::test(start_paused = true)]
async fn keeps_retrying_until_it_reads_again() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    invalidations.refuse(InvalidationStep::Stream);
    invalidations.refuse(InvalidationStep::Consumer);
    invalidations.refuse(InvalidationStep::Messages);

    invalidations.fail();
    for _ in 0..3 {
        retry().await;
        assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
    }
    retry().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    assert!(invalidations.is_read());
    assert_eq!(invalidations.streams().len(), 5);
    let configs = invalidations.configs();
    assert_eq!(configs.len(), 4);
    assert!(
        configs
            .iter()
            .all(|config| config.deliver_policy == configs[0].deliver_policy)
    );
}

#[tokio::test(start_paused = true)]
async fn holds_releases_again_once_it_reads_again() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 1);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());

    invalidations.fail();
    retry().await;
    client.release(&asked).await.unwrap();
    client.release(&asked).await.unwrap();

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    assert_eq!(fake.received().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn serves_a_held_release_without_asking_again() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 1);
    let client = started(&fake, &ScriptedInvalidations::default()).await.unwrap();
    let asked = request(&tenant());

    let first = client.release(&asked).await.unwrap();
    let second = client.clone().release(&asked).await.unwrap();

    assert_eq!(first.value().as_str(), VALUE);
    assert_eq!(second.value().as_str(), VALUE);
    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    assert_eq!(fake.received().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn stops_reading_once_the_last_clone_is_dropped() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    let clone = client.clone();

    drop(client);
    settle().await;
    assert!(invalidations.is_read());

    drop(clone);
    settle().await;
    assert!(!invalidations.is_read());
}

#[tokio::test(start_paused = true)]
async fn asks_again_once_its_hold_passes_even_with_the_wall_clock_moved_back() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 2);
    let clock = SteppedClock::at(now());
    let client = started_on(&fake, &ScriptedInvalidations::default(), clock.clone())
        .await
        .unwrap();
    let asked = request(&tenant());
    client.release(&asked).await.unwrap();

    clock.advance(in_a_minute().duration_since(now()).unwrap());
    clock.set_wall(now() - Duration::from_secs(3_600));
    client.release(&asked).await.unwrap();

    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn fails_closed_when_both_releases_are_invalidated_on_their_way() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, REVOKED);
    releasing_late(&fake, REVOKED);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());
    let subject = notice_subject(asked.tenant(), CREDENTIAL_ID);

    let released = releasing_in_background(&client, &asked).await;
    invalidations.notify(&subject);
    reply().await;
    invalidations.notify(&subject);
    reply().await;
    let refused = released.await.unwrap().unwrap_err();

    assert!(matches!(refused, ReleaseError::Invalidated));
    assert_eq!(refused.to_string(), "release was invalidated while it was on its way");
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn asks_again_when_the_credential_is_invalidated_while_its_release_is_on_its_way() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, REVOKED);
    releasing(&fake, 1);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());

    let released = releasing_in_background(&client, &asked).await;
    invalidations.notify(&notice_subject(asked.tenant(), CREDENTIAL_ID));
    reply().await;
    let released = released.await.unwrap().unwrap();
    let held = client.release(&asked).await.unwrap();

    assert_eq!(released.value().as_str(), VALUE);
    assert_eq!(held.value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn asks_again_when_its_reader_loses_its_place_while_a_release_is_on_its_way() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, REVOKED);
    releasing(&fake, 1);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());

    let released = releasing_in_background(&client, &asked).await;
    invalidations.fail();
    reply().await;

    assert_eq!(released.await.unwrap().unwrap().value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn keeps_a_release_on_its_way_when_a_notice_names_another_credential() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, VALUE);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());

    let released = releasing_in_background(&client, &asked).await;
    invalidations.notify(&notice_subject(asked.tenant(), OTHER_CREDENTIAL_ID));
    reply().await;
    let released = released.await.unwrap().unwrap();
    let held = client.release(&asked).await.unwrap();

    assert_eq!(released.value().as_str(), VALUE);
    assert_eq!(held.value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn keeps_its_place_while_quiet_and_caught_up() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();

    next_position_check().await;
    settle().await;
    next_position_check().await;
    settle().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    assert_eq!(invalidations.position_checks(), 2);
}

#[tokio::test(start_paused = true)]
async fn keeps_its_place_when_a_delivered_notice_arrives_before_its_next_look() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    invalidations.hold_back(&notice_subject(&tenant(), OTHER_CREDENTIAL_ID));

    next_position_check().await;
    settle().await;
    invalidations.deliver_held_back();
    next_position_check().await;
    settle().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    assert_eq!(invalidations.position_checks(), 1);
}

#[tokio::test(start_paused = true)]
async fn asks_nothing_of_its_consumer_while_notices_keep_arriving() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    let subject = notice_subject(&tenant(), OTHER_CREDENTIAL_ID);

    for _ in 0..20 {
        invalidations.notify(&subject);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    assert_eq!(invalidations.position_checks(), 0);
}

#[tokio::test(start_paused = true)]
async fn serves_uncached_once_a_delivered_notice_never_arrives() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 2);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());
    client.release(&asked).await.unwrap();
    invalidations.hold_back(&notice_subject(asked.tenant(), CREDENTIAL_ID));

    next_position_check().await;
    settle().await;
    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    next_position_check().await;
    settle().await;
    client.release(&asked).await.unwrap();

    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn serves_uncached_once_it_cannot_tell_how_far_its_consumer_delivered() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    invalidations.refuse(InvalidationStep::Position);

    next_position_check().await;
    settle().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
}

#[tokio::test(start_paused = true)]
async fn records_no_loss_while_notices_arrive_in_order() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::INFO);
    let recorded = RecordedMetrics::new();
    let invalidations = ScriptedInvalidations::default();
    let client = started_recording(
        &FakeSecretsService::new(),
        &invalidations,
        SteppedClock::at(now()),
        1,
        &recorded,
    )
    .await;
    let subject = notice_subject(&tenant(), OTHER_CREDENTIAL_ID);

    invalidations.notify(&subject);
    invalidations.notify(&subject);
    next_position_check().await;
    settle().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    assert_eq!(events.events(), []);
    assert!(recorded.counts(SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES).is_empty());
}

#[tokio::test(start_paused = true)]
async fn warns_and_counts_when_its_consumer_fails() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let recorded = RecordedMetrics::new();
    let invalidations = ScriptedInvalidations::default();
    let _client = started_recording(
        &FakeSecretsService::new(),
        &invalidations,
        SteppedClock::at(now()),
        1,
        &recorded,
    )
    .await;

    invalidations.fail();
    settle().await;

    assert_eq!(reasons_warned(&events), ["failed"]);
    assert_eq!(
        recorded.counts(SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES),
        losses_counted("failed")
    );
}

#[tokio::test(start_paused = true)]
async fn warns_and_counts_when_a_notice_is_skipped() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let recorded = RecordedMetrics::new();
    let invalidations = ScriptedInvalidations::default();
    let _client = started_recording(
        &FakeSecretsService::new(),
        &invalidations,
        SteppedClock::at(now()),
        1,
        &recorded,
    )
    .await;

    invalidations.skip_one(&notice_subject(&tenant(), OTHER_CREDENTIAL_ID));
    settle().await;

    assert_eq!(
        losses_warned(&events),
        [("gap".to_owned(), "notice 2 arrived where notice 1 was due".to_owned())]
    );
    assert_eq!(
        recorded.counts(SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES),
        losses_counted("gap")
    );
}

#[tokio::test(start_paused = true)]
async fn warns_and_counts_when_a_notice_cannot_be_placed() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let recorded = RecordedMetrics::new();
    let invalidations = ScriptedInvalidations::default();
    let _client = started_recording(
        &FakeSecretsService::new(),
        &invalidations,
        SteppedClock::at(now()),
        1,
        &recorded,
    )
    .await;

    invalidations.notify_unplaced(&notice_subject(&tenant(), OTHER_CREDENTIAL_ID));
    settle().await;

    assert_eq!(
        losses_warned(&events),
        [(
            "unplaced".to_owned(),
            "a notice arrived without its place in the delivery: fake-no-delivery-info".to_owned()
        )]
    );
    assert_eq!(
        recorded.counts(SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES),
        losses_counted("unplaced")
    );
}

#[tokio::test(start_paused = true)]
async fn warns_and_counts_when_its_feed_ends() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let recorded = RecordedMetrics::new();
    let invalidations = ScriptedInvalidations::default();
    let _client = started_recording(
        &FakeSecretsService::new(),
        &invalidations,
        SteppedClock::at(now()),
        1,
        &recorded,
    )
    .await;

    invalidations.end();
    settle().await;

    assert_eq!(
        losses_warned(&events),
        [(
            "ended".to_owned(),
            "the invalidation consumer stopped delivering notices".to_owned()
        )]
    );
    assert_eq!(
        recorded.counts(SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES),
        losses_counted("ended")
    );
}

#[tokio::test(start_paused = true)]
async fn warns_and_counts_when_a_delivered_notice_never_arrives() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let recorded = RecordedMetrics::new();
    let invalidations = ScriptedInvalidations::default();
    let _client = started_recording(
        &FakeSecretsService::new(),
        &invalidations,
        SteppedClock::at(now()),
        1,
        &recorded,
    )
    .await;
    invalidations.hold_back(&notice_subject(&tenant(), OTHER_CREDENTIAL_ID));

    next_position_check().await;
    settle().await;
    assert_eq!(losses_warned(&events), []);
    next_position_check().await;
    settle().await;

    assert_eq!(
        losses_warned(&events),
        [(
            "missing".to_owned(),
            "notices through 1 were delivered, but only through 0 arrived".to_owned()
        )]
    );
    assert_eq!(
        recorded.counts(SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES),
        losses_counted("missing")
    );
}

#[tokio::test(start_paused = true)]
async fn warns_and_counts_when_it_cannot_tell_how_far_its_consumer_delivered() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let recorded = RecordedMetrics::new();
    let invalidations = ScriptedInvalidations::default();
    let _client = started_recording(
        &FakeSecretsService::new(),
        &invalidations,
        SteppedClock::at(now()),
        1,
        &recorded,
    )
    .await;
    invalidations.refuse(InvalidationStep::Position);

    next_position_check().await;
    settle().await;

    assert_eq!(reasons_warned(&events), ["position_unknown"]);
    assert_eq!(
        recorded.counts(SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES),
        losses_counted("position_unknown")
    );
}

#[tokio::test(start_paused = true)]
async fn warns_once_per_loss_however_long_it_takes_to_read_again() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let recorded = RecordedMetrics::new();
    let invalidations = ScriptedInvalidations::default();
    let _client = started_recording(
        &FakeSecretsService::new(),
        &invalidations,
        SteppedClock::at(now()),
        1,
        &recorded,
    )
    .await;
    invalidations.refuse(InvalidationStep::Stream);
    invalidations.refuse(InvalidationStep::Consumer);

    invalidations.fail();
    for _ in 0..3 {
        retry().await;
    }

    assert_eq!(events.events().len(), 1);
    assert_eq!(reasons_warned(&events), ["failed"]);
    assert_eq!(
        recorded.counts(SECRETS_CLIENT_INVALIDATION_POSITION_LOSSES),
        losses_counted("failed")
    );
}

#[tokio::test(start_paused = true)]
async fn says_how_many_attempts_it_took_to_read_again() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::INFO);
    let invalidations = ScriptedInvalidations::default();
    let client = started_recording(
        &FakeSecretsService::new(),
        &invalidations,
        SteppedClock::at(now()),
        1,
        &RecordedMetrics::new(),
    )
    .await;
    invalidations.refuse(InvalidationStep::Stream);

    invalidations.fail();
    retry().await;
    assert!(events.events().iter().all(|event| event.message() != Some(READS_AGAIN)));
    retry().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    let read_again: Vec<_> = events
        .events()
        .into_iter()
        .filter(|event| event.message() == Some(READS_AGAIN))
        .collect();
    assert_eq!(read_again.len(), 1);
    assert_eq!(read_again[0].field("attempts"), Some("2"));
}

#[tokio::test(start_paused = true)]
async fn records_nothing_while_the_cache_has_room() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let recorded = RecordedMetrics::new();
    let fake = FakeSecretsService::new();
    releasing_for_an_hour(&fake, 1);
    let client = started_recording(
        &fake,
        &ScriptedInvalidations::default(),
        SteppedClock::at(now()),
        1,
        &recorded,
    )
    .await;
    let asked = request(&tenant());

    client.release(&asked).await.unwrap();
    client.release(&asked).await.unwrap();

    assert_eq!(fake.received().len(), 1);
    assert_eq!(events.events(), []);
    assert!(recorded.counts(SECRETS_CLIENT_RELEASE_CACHE_CAPACITY_SKIPS).is_empty());
}

#[tokio::test(start_paused = true)]
async fn counts_every_release_served_without_being_held_but_warns_once_per_interval() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let recorded = RecordedMetrics::new();
    let fake = FakeSecretsService::new();
    releasing_for_an_hour(&fake, 5);
    let clock = SteppedClock::at(now());
    let client = started_recording(&fake, &ScriptedInvalidations::default(), clock.clone(), 1, &recorded).await;
    let held = request(&tenant());
    let turned_away = of_credential(&held, OTHER_CREDENTIAL_ID);
    client.release(&held).await.unwrap();

    for _ in 0..3 {
        client.release(&turned_away).await.unwrap();
    }
    let warned_once: Vec<_> = events.events();
    clock.advance(FULL_CACHE_WARNING_INTERVAL);
    client.release(&turned_away).await.unwrap();

    assert_eq!(fake.received().len(), 5);
    assert_eq!(
        recorded.counts(SECRETS_CLIENT_RELEASE_CACHE_CAPACITY_SKIPS),
        [(Vec::<String>::new(), 4)]
    );
    assert_eq!(warned_once.len(), 1);
    let warnings: Vec<_> = events
        .events()
        .into_iter()
        .filter(|event| event.message() == Some(CACHE_FULL))
        .map(|event| {
            (
                event.field("capacity").unwrap().to_owned(),
                event.field("turned_away").unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        warnings,
        [("1".to_owned(), "1".to_owned()), ("1".to_owned(), "3".to_owned())]
    );
}

#[tokio::test(start_paused = true)]
async fn fails_every_waiter_with_the_call_they_shared_and_asks_again_at_the_next_miss() {
    let fake = FakeSecretsService::new();
    fake.script(FakeOutcome::invalid_argument("tenant unknown").after(REPLY_DELAY));
    releasing(&fake, 1);
    let client = started(&fake, &ScriptedInvalidations::default()).await.unwrap();
    let asked = request(&tenant());

    let mut waiting = Vec::new();
    for _ in 0..3 {
        waiting.push(releasing_in_background(&client, &asked).await);
    }
    reply().await;
    for released in waiting {
        let ReleaseError::InvalidArgument(fault) = released.await.unwrap().unwrap_err() else {
            panic!("every waiter fails as the call they shared did");
        };
        assert_eq!(fault.message(), "tenant unknown");
    }
    assert_eq!(fake.received().len(), 1);

    assert_eq!(client.release(&asked).await.unwrap().value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn neither_holds_nor_returns_a_shared_call_invalidated_on_its_way() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, REVOKED);
    releasing(&fake, 1);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());

    let mut waiting = Vec::new();
    for _ in 0..3 {
        waiting.push(releasing_in_background(&client, &asked).await);
    }
    invalidations.notify(&notice_subject(asked.tenant(), CREDENTIAL_ID));
    reply().await;
    for released in waiting {
        assert_eq!(released.await.unwrap().unwrap().value().as_str(), VALUE);
    }
    let held = client.release(&asked).await.unwrap();

    assert_eq!(held.value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn does_not_wait_on_a_call_an_invalidation_already_overtook() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, REVOKED);
    releasing(&fake, 1);
    let invalidations = ScriptedInvalidations::default();
    let client = started(&fake, &invalidations).await.unwrap();
    let asked = request(&tenant());

    let overtaken = releasing_in_background(&client, &asked).await;
    invalidations.notify(&notice_subject(asked.tenant(), CREDENTIAL_ID));
    settle().await;
    let asked_at = Instant::now();
    let released = client.release(&asked).await.unwrap();

    assert_eq!(asked_at.elapsed(), Duration::ZERO);
    assert_eq!(released.value().as_str(), VALUE);
    reply().await;
    assert_eq!(overtaken.await.unwrap().unwrap().value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn asks_the_service_once_for_concurrent_misses_of_one_key() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, VALUE);
    let client = started(&fake, &ScriptedInvalidations::default()).await.unwrap();
    let asked = request(&tenant());

    let mut waiting = Vec::new();
    for _ in 0..5 {
        waiting.push(releasing_in_background(&client, &asked).await);
    }
    reply().await;

    for released in waiting {
        assert_eq!(released.await.unwrap().unwrap().value().as_str(), VALUE);
    }
    assert_eq!(client.release(&asked).await.unwrap().value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn keeps_the_call_going_for_its_waiters_when_the_caller_that_started_it_leaves() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, VALUE);
    let client = started(&fake, &ScriptedInvalidations::default()).await.unwrap();
    let asked = request(&tenant());

    let starter = releasing_in_background(&client, &asked).await;
    let waiter = releasing_in_background(&client, &asked).await;
    starter.abort();
    reply().await;

    assert!(starter.await.unwrap_err().is_cancelled());
    assert_eq!(waiter.await.unwrap().unwrap().value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn holds_what_a_call_released_after_every_caller_left_it() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, VALUE);
    let client = started(&fake, &ScriptedInvalidations::default()).await.unwrap();
    let asked = request(&tenant());

    releasing_in_background(&client, &asked).await.abort();
    reply().await;

    assert_eq!(client.release(&asked).await.unwrap().value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn lets_each_waiter_stop_at_its_own_deadline() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, VALUE);
    let client = started(&fake, &ScriptedInvalidations::default()).await.unwrap();
    let asked = request(&tenant());

    let patient = releasing_in_background(&client, &asked).await;
    let (impatient_client, impatient_asked) = (client.clone(), asked.clone());
    let impatient =
        tokio::spawn(
            async move { tokio::time::timeout(REPLY_DELAY / 2, impatient_client.release(&impatient_asked)).await },
        );

    assert!(impatient.await.unwrap().is_err());
    assert_eq!(patient.await.unwrap().unwrap().value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn asks_for_distinct_keys_without_one_waiting_on_the_other() {
    let fake = FakeSecretsService::new();
    releasing_late(&fake, VALUE);
    releasing(&fake, 1);
    let client = started(&fake, &ScriptedInvalidations::default()).await.unwrap();
    let asked = request(&tenant());
    let other = of_credential(&asked, OTHER_CREDENTIAL_ID);

    let late = releasing_in_background(&client, &asked).await;
    let asked_at = Instant::now();
    let released = client.release(&other).await.unwrap();

    assert_eq!(asked_at.elapsed(), Duration::ZERO);
    assert_eq!(released.value().as_str(), VALUE);
    reply().await;
    assert_eq!(late.await.unwrap().unwrap().value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 2);
}

/// Starts a client in the background, as one whose start has not returned.
fn starting(
    fake: &FakeSecretsService,
    invalidations: &ScriptedInvalidations,
) -> JoinHandle<Result<Client, InvalidationStartError>> {
    let (fake, invalidations) = (fake.clone(), invalidations.clone());
    tokio::spawn(async move { started(&fake, &invalidations).await })
}

#[tokio::test(start_paused = true)]
async fn does_not_start_until_its_backlog_is_drained() {
    let invalidations = ScriptedInvalidations::default();
    invalidations.replay(2);
    let starting = starting(&FakeSecretsService::new(), &invalidations);
    let subject = notice_subject(&tenant(), OTHER_CREDENTIAL_ID);
    settle().await;
    assert!(!starting.is_finished());

    invalidations.notify(&subject);
    settle().await;
    assert!(!starting.is_finished());
    invalidations.notify(&subject);
    settle().await;

    assert!(starting.is_finished());
    assert_eq!(starting.await.unwrap().unwrap().mode(), ReleaseCacheMode::Caching);
}

#[tokio::test(start_paused = true)]
async fn stops_reading_when_starting_is_given_up_before_its_backlog_is_drained() {
    let invalidations = ScriptedInvalidations::default();
    invalidations.replay(2);
    let starting = starting(&FakeSecretsService::new(), &invalidations);
    settle().await;
    assert!(invalidations.is_read());

    starting.abort();
    settle().await;

    assert!(!invalidations.is_read());
}

#[tokio::test(start_paused = true)]
async fn starts_at_once_when_the_stream_holds_no_backlog() {
    let starting = starting(&FakeSecretsService::new(), &ScriptedInvalidations::default());
    settle().await;

    assert!(starting.is_finished());
    assert_eq!(starting.await.unwrap().unwrap().mode(), ReleaseCacheMode::Caching);
}

#[tokio::test(start_paused = true)]
async fn releases_on_the_first_call_once_started_however_many_notices_replayed() {
    let fake = FakeSecretsService::new();
    releasing(&fake, 1);
    let invalidations = ScriptedInvalidations::default();
    let replayed = 2_000;
    invalidations.replay(replayed);
    let starting = starting(&fake, &invalidations);
    settle().await;
    let subject = notice_subject(&tenant(), CREDENTIAL_ID);
    for _ in 0..replayed {
        invalidations.notify(&subject);
    }
    let client = starting.await.unwrap().unwrap();

    let released = client.release(&request(&tenant())).await.unwrap();

    assert_eq!(released.value().as_str(), VALUE);
    assert_eq!(fake.received().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn starts_uncached_when_it_loses_its_place_while_catching_up() {
    let invalidations = ScriptedInvalidations::default();
    invalidations.replay(2);
    let starting = starting(&FakeSecretsService::new(), &invalidations);
    settle().await;

    invalidations.notify(&notice_subject(&tenant(), OTHER_CREDENTIAL_ID));
    invalidations.fail();
    settle().await;

    assert!(starting.is_finished());
    assert_eq!(starting.await.unwrap().unwrap().mode(), ReleaseCacheMode::Uncached);
}

#[tokio::test(start_paused = true)]
async fn stays_uncached_until_its_new_consumer_has_drained_its_backlog() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    let subject = notice_subject(&tenant(), OTHER_CREDENTIAL_ID);
    invalidations.replay(2);
    invalidations.fail();

    retry().await;
    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
    invalidations.notify(&subject);
    settle().await;
    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
    invalidations.notify(&subject);
    settle().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
}

#[tokio::test(start_paused = true)]
async fn stays_uncached_when_it_loses_its_place_again_before_catching_up() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    invalidations.replay(2);
    invalidations.fail();
    retry().await;

    invalidations.notify(&notice_subject(&tenant(), OTHER_CREDENTIAL_ID));
    invalidations.fail();
    settle().await;
    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);
    retry().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
    assert_eq!(invalidations.configs().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn starts_once_a_quiet_look_finds_nothing_owed_though_its_backlog_was_larger() {
    let invalidations = ScriptedInvalidations::default();
    invalidations.replay(3);
    let starting = starting(&FakeSecretsService::new(), &invalidations);
    settle().await;
    invalidations.notify(&notice_subject(&tenant(), OTHER_CREDENTIAL_ID));
    settle().await;
    assert!(!starting.is_finished());

    next_position_check().await;
    settle().await;
    next_position_check().await;
    settle().await;

    assert!(starting.is_finished());
    assert_eq!(starting.await.unwrap().unwrap().mode(), ReleaseCacheMode::Caching);
}

#[tokio::test(start_paused = true)]
async fn holds_releases_again_once_a_quiet_look_finds_nothing_owed_though_its_backlog_was_larger() {
    let invalidations = ScriptedInvalidations::default();
    let client = started(&FakeSecretsService::new(), &invalidations).await.unwrap();
    invalidations.replay(3);
    invalidations.fail();
    retry().await;
    assert_eq!(client.mode(), ReleaseCacheMode::Uncached);

    next_position_check().await;
    settle().await;
    next_position_check().await;
    settle().await;

    assert_eq!(client.mode(), ReleaseCacheMode::Caching);
}

#[tokio::test(start_paused = true)]
async fn keeps_waiting_while_a_quiet_look_finds_a_notice_still_owed() {
    let invalidations = ScriptedInvalidations::default();
    invalidations.replay(2);
    let starting = starting(&FakeSecretsService::new(), &invalidations);
    settle().await;
    invalidations.hold_back(&notice_subject(&tenant(), OTHER_CREDENTIAL_ID));

    next_position_check().await;
    settle().await;

    assert!(!starting.is_finished());
}
