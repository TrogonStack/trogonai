use std::time::Duration;

use async_nats::jetstream::stream::ClusterInfo;
use tokio::time::{MissedTickBehavior, interval};

#[cfg(test)]
mod tests;

pub(super) async fn wait_for_quorum(mut observe: impl AsyncFnMut() -> Option<ClusterInfo>) {
    let mut polls = interval(Duration::from_millis(50));
    polls.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        polls.tick().await;
        if observe().await.as_ref().is_some_and(|cluster| {
            cluster.leader.is_some()
                && cluster.replicas.len() == 2
                && cluster.replicas.iter().all(|peer| peer.current && !peer.offline)
        }) {
            break;
        }
    }
}
