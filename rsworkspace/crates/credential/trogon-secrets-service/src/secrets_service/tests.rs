use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::until_either_stops;

const STOPPED_WITHIN: Duration = Duration::from_secs(5);

/// A part that runs until it is told to stop.
async fn runs_until(stop: CancellationToken) -> Result<(), &'static str> {
    stop.cancelled().await;
    Ok(())
}

#[tokio::test]
async fn a_publisher_that_stops_on_its_own_stops_the_release_endpoint() {
    let shutdown = CancellationToken::new();

    let stopped = tokio::time::timeout(
        STOPPED_WITHIN,
        until_either_stops(&shutdown, runs_until, |_| async {
            Err::<(), _>("fake-publisher-stopped")
        }),
    )
    .await
    .expect("the release endpoint kept serving after the publisher stopped");

    assert_eq!(stopped, (Ok(()), Err("fake-publisher-stopped")));
    assert!(!shutdown.is_cancelled(), "a part stopping is not a shutdown");
}

#[tokio::test]
async fn a_release_endpoint_that_stops_on_its_own_stops_the_publisher() {
    let shutdown = CancellationToken::new();

    let stopped = tokio::time::timeout(
        STOPPED_WITHIN,
        until_either_stops(
            &shutdown,
            |_| async { Err::<(), _>("fake-endpoint-stopped") },
            runs_until,
        ),
    )
    .await
    .expect("the publisher kept publishing after the release endpoint stopped");

    assert_eq!(stopped, (Err("fake-endpoint-stopped"), Ok(())));
}

#[tokio::test]
async fn shutdown_stops_the_release_endpoint_and_the_publisher() {
    let shutdown = CancellationToken::new();
    shutdown.cancel();

    let stopped = tokio::time::timeout(STOPPED_WITHIN, until_either_stops(&shutdown, runs_until, runs_until))
        .await
        .expect("a part kept running after shutdown");

    assert_eq!(stopped, (Ok(()), Ok(())));
}
