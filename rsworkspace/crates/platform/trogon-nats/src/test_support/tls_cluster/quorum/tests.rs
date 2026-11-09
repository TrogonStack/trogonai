use std::collections::VecDeque;
use std::time::Duration;

use async_nats::jetstream::stream::{ClusterInfo, PeerInfo};
use tokio::time::{Instant, timeout};

use super::wait_for_quorum;

#[tokio::test(start_paused = true)]
async fn absent_or_incomplete_observations_cannot_complete_quorum_readiness() {
    let ready = ClusterInfo {
        leader: Some("leader".into()),
        replicas: vec![
            PeerInfo {
                name: "replica-a".into(),
                current: true,
                ..PeerInfo::default()
            },
            PeerInfo {
                name: "replica-b".into(),
                current: true,
                ..PeerInfo::default()
            },
        ],
        ..ClusterInfo::default()
    };
    let mut leaderless = ready.clone();
    leaderless.leader = None;
    let mut too_few = ready.clone();
    too_few.replicas.pop();
    let mut too_many = ready.clone();
    too_many.replicas.push(ready.replicas[0].clone());
    let mut stale = ready.clone();
    stale.replicas[0].current = false;
    let mut offline = ready.clone();
    offline.replicas[1].offline = true;
    let mut observations = VecDeque::from([
        None,
        Some(leaderless),
        Some(too_few),
        Some(too_many),
        Some(stale),
        Some(offline),
        Some(ready),
    ]);
    let mut calls = 0;
    let started = Instant::now();
    timeout(
        Duration::from_secs(2),
        wait_for_quorum(async || {
            calls += 1;
            observations
                .pop_front()
                .expect("quorum probe must stop at the ready observation")
        }),
    )
    .await
    .expect("controlled readiness observations complete within the bound");
    assert_eq!(calls, 7);
    assert!(observations.is_empty());
    assert_eq!(started.elapsed(), Duration::from_millis(300));
}

#[tokio::test(start_paused = true)]
async fn unavailable_observations_remain_pending_without_a_busy_loop() {
    let mut calls = 0;
    let started = Instant::now();
    let result = timeout(
        Duration::from_millis(125),
        wait_for_quorum(async || {
            calls += 1;
            None
        }),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(calls, 3);
    assert_eq!(started.elapsed(), Duration::from_millis(125));
}
