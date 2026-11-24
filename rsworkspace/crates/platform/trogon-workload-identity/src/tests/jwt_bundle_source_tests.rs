use std::time::UNIX_EPOCH;

use serde_json::json;

use super::jwt_svid_tests::{AUD, FixedClock, NOW, TestKey, claims, ec_key, ec_token, hop};
use super::*;
use crate::constants::JWT_BUNDLE_MAX_STALENESS;
use crate::jwt_bundles_wire::JwtBundlesWire;

fn jwks(keys: &[&TestKey]) -> String {
    json!({ "keys": keys.iter().map(|key| key.jwk.clone()).collect::<Vec<_>>() }).to_string()
}

fn verifier_over(source: JwtBundleSource) -> JwtSvidVerifier {
    JwtSvidVerifier::new(JwtAudience::new(AUD).unwrap(), source)
        .with_clock(Arc::new(FixedClock(UNIX_EPOCH + Duration::from_secs(NOW))))
}

fn wire(bundles: &[(&str, String)]) -> JwtBundlesWire {
    JwtBundlesWire {
        bundles: bundles
            .iter()
            .map(|(domain, jwks)| ((*domain).to_owned(), jwks.as_bytes().to_vec()))
            .collect(),
    }
}

fn verdict(verifier: &JwtSvidVerifier, key: &TestKey) -> Result<(), JwtSvidError> {
    verifier.verify(&ec_token(key, &claims()), &hop()).map(drop)
}

async fn running_source(
    fixture: &WorkloadApiFixture,
) -> (JwtBundleSource, CancellationToken, tokio::task::JoinHandle<()>) {
    let (source, driver) = JwtBundleSource::new(fixture.endpoint());
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(driver.run(shutdown.clone()));
    (source, shutdown, task)
}

#[tokio::test]
async fn nothing_verifies_before_the_first_set_arrives_over_the_workload_api() {
    let key = ec_key("a");
    let fixture = WorkloadApiFixture::start(Vec::new()).await;
    let (source, shutdown, task) = running_source(&fixture).await;
    let verifier = verifier_over(source);
    until(|| fixture.jwt_bundle_requests() >= 1).await;
    assert_eq!(verdict(&verifier, &key), Err(JwtSvidError::BundlesNotDelivered));
    assert_eq!(JwtSvidError::BundlesNotDelivered.code(), "bundles_not_delivered");
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn a_token_signed_by_a_newly_delivered_authority_verifies_after_the_update() {
    let (old, new) = (ec_key("old"), ec_key("new"));
    let fixture = WorkloadApiFixture::start(Vec::new()).await;
    let (source, shutdown, task) = running_source(&fixture).await;
    let verifier = verifier_over(source);
    fixture.replace_jwt_bundles(&[("example.test", &jwks(&[&old]))]);
    until(|| verdict(&verifier, &old).is_ok()).await;
    assert_eq!(verdict(&verifier, &new), Err(JwtSvidError::UnknownKeyId));
    fixture.replace_jwt_bundles(&[("example.test", &jwks(&[&old, &new]))]);
    until(|| verdict(&verifier, &new).is_ok()).await;
    assert!(verdict(&verifier, &old).is_ok());
    assert_eq!(fixture.jwt_bundle_requests(), 1, "rotation arrives on the open stream");
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn a_token_signed_by_an_authority_removed_in_an_update_is_refused_after_the_update() {
    let (old, new) = (ec_key("old"), ec_key("new"));
    let fixture = WorkloadApiFixture::start(Vec::new()).await;
    let (source, shutdown, task) = running_source(&fixture).await;
    let verifier = verifier_over(source);
    fixture.replace_jwt_bundles(&[("example.test", &jwks(&[&old, &new]))]);
    until(|| verdict(&verifier, &old).is_ok()).await;
    fixture.replace_jwt_bundles(&[("example.test", &jwks(&[&new]))]);
    until(|| verdict(&verifier, &old) == Err(JwtSvidError::UnknownKeyId)).await;
    assert!(verdict(&verifier, &new).is_ok());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn an_invalid_update_is_refused_and_the_last_good_set_is_kept() {
    let (good, other) = (ec_key("good"), ec_key("other"));
    let fixture = WorkloadApiFixture::start(Vec::new()).await;
    let (source, shutdown, task) = running_source(&fixture).await;
    let verifier = verifier_over(source);
    fixture.replace_jwt_bundles(&[("example.test", &jwks(&[&good]))]);
    until(|| verdict(&verifier, &good).is_ok()).await;
    let before = fixture.jwt_bundle_requests();
    fixture.replace_jwt_bundles(&[("example.test", &jwks(&[])), ("other.test", jwks(&[&other]).as_str())]);
    until(|| fixture.jwt_bundle_requests() > before).await;
    assert!(
        verdict(&verifier, &good).is_ok(),
        "the refused update must not erase the set"
    );
    assert_eq!(verdict(&verifier, &other), Err(JwtSvidError::UnknownKeyId));
    fixture.replace_jwt_bundles(&[("example.test", &jwks(&[&other]))]);
    until(|| verdict(&verifier, &other).is_ok()).await;
    assert_eq!(verdict(&verifier, &good), Err(JwtSvidError::UnknownKeyId));
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn a_denied_stream_keeps_the_last_set_and_reconnects() {
    let key = ec_key("a");
    let fixture = WorkloadApiFixture::start(Vec::new()).await;
    let (source, shutdown, task) = running_source(&fixture).await;
    let verifier = verifier_over(source);
    fixture.replace_jwt_bundles(&[("example.test", &jwks(&[&key]))]);
    until(|| verdict(&verifier, &key).is_ok()).await;
    let before = fixture.jwt_bundle_requests();
    fixture.jwt_bundles_permission_denied();
    until(|| fixture.jwt_bundle_requests() > before).await;
    assert!(verdict(&verifier, &key).is_ok());
    shutdown.cancel();
    task.await.unwrap();
    fixture.stop().await;
}

#[tokio::test(start_paused = true)]
async fn the_verifier_fails_closed_with_its_own_code_once_the_stream_has_been_lost_too_long() {
    let key = ec_key("a");
    let (source, driver) = JwtBundleSource::new(UnixEndpoint::new("unix:///nonexistent/workload.sock").unwrap());
    let verifier = verifier_over(source.clone());
    source
        .state
        .replace(JwtBundleSet::from_wire(wire(&[("example.test", jwks(&[&key]))])).unwrap());
    assert!(verdict(&verifier, &key).is_ok());
    source.state.lost();
    tokio::time::advance(JWT_BUNDLE_MAX_STALENESS).await;
    assert!(verdict(&verifier, &key).is_ok());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(verdict(&verifier, &key), Err(JwtSvidError::BundlesStale));
    assert_eq!(JwtSvidError::BundlesStale.code(), "bundles_stale");
    drop(driver);
}

#[test]
fn a_key_of_one_trust_domain_never_verifies_a_token_of_another_in_a_delivered_set() {
    let (mine, theirs) = (ec_key("same"), ec_key("same"));
    let set = JwtBundleSet::from_wire(wire(&[
        ("example.test", jwks(&[&mine])),
        ("other.test", jwks(&[&theirs])),
    ]))
    .unwrap();
    assert_eq!(set.trust_domains().count(), 2);
    let verifier = verifier_over(JwtBundleSource::static_for_tests(set).unwrap());
    assert_eq!(verdict(&verifier, &theirs), Err(JwtSvidError::InvalidSignature));
    assert!(verdict(&verifier, &mine).is_ok());
}

#[test]
fn an_update_is_refused_whole_when_any_entry_is_invalid() {
    let key = ec_key("a");
    let good = jwks(&[&key]);
    for (update, expected) in [
        (wire(&[]), JwtBundleError::NoBundles),
        (
            wire(&[("example.test", good.clone()), ("other.test", jwks(&[]))]),
            JwtBundleError::NoAuthorities,
        ),
        (
            wire(&[("example.test", good.clone()), ("not a domain", good.clone())]),
            JwtBundleError::InvalidTrustDomain,
        ),
        (
            wire(&[("example.test", "not json".to_owned())]),
            JwtBundleError::Malformed,
        ),
    ] {
        assert_eq!(JwtBundleSet::from_wire(update).unwrap_err(), expected);
    }
    let many: Vec<_> = (0..=crate::constants::MAX_ROOTS)
        .map(|index| (format!("domain{index}.test"), good.clone()))
        .collect();
    let many: Vec<_> = many
        .iter()
        .map(|(domain, jwks)| (domain.as_str(), jwks.clone()))
        .collect();
    assert_eq!(
        JwtBundleSet::from_wire(wire(&many)).unwrap_err(),
        JwtBundleError::TooManyBundles
    );
}
