use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull};
use futures::{Stream, StreamExt};
use time::OffsetDateTime;
use tokio::task::AbortHandle;
use trogon_std::time::EpochClock;

use super::constants::{INVALIDATION_FILTER, INVALIDATION_LOOKBACK, INVALIDATION_RETRY_DELAY, INVALIDATION_STREAM};
use super::{
    InvalidationConsumer, InvalidationNotice, InvalidationStartError, InvalidationStream, InvalidationStreamSource,
    ReleaseCache, ReleaseInvalidation,
};

type Notices<J> =
    <<<J as InvalidationStreamSource>::Stream as InvalidationStream>::Consumer as InvalidationConsumer>::Notices;

/// One client's own reader of release invalidations, read the way an ordered
/// consumer reads: an ephemeral consumer of its own, never durable and never
/// in a queue group, so every replica hears every notice (ADR#0023 Decision
/// 6), with no acks, checked to arrive one after another.
///
/// It starts [`INVALIDATION_LOOKBACK`] back, so a notice sent while it was
/// not reading is still heard. It loses its place on any error, any gap in
/// what it was delivered, or the end of its consumer; then it drops every
/// held copy and holds none, retrying every [`INVALIDATION_RETRY_DELAY`]
/// until it reads again. It stops when dropped.
///
/// The client's own `pull::Ordered` is not used: it recreates its consumer
/// from where it was without saying so, so a stream that was replaced, or
/// a consumer gone for longer than its notices are kept, would skip notices
/// unseen.
#[derive(Debug)]
pub(crate) struct InvalidationReader(AbortHandle);

impl InvalidationReader {
    pub(crate) async fn start<J, C>(
        jetstream: J,
        clock: C,
        cache: Arc<Mutex<ReleaseCache>>,
    ) -> Result<Self, InvalidationStartError>
    where
        J: InvalidationStreamSource,
        C: EpochClock,
    {
        let notices = subscribe(&jetstream, &clock).await?;
        let task = tokio::spawn(read(jetstream, clock, cache, notices));
        Ok(Self(task.abort_handle()))
    }
}

impl Drop for InvalidationReader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn subscribe<J, C>(jetstream: &J, clock: &C) -> Result<Notices<J>, InvalidationStartError>
where
    J: InvalidationStreamSource,
    C: EpochClock,
{
    let stream = jetstream.get_stream(INVALIDATION_STREAM).await?;
    let consumer = stream.create_consumer(consumer_config(clock.system_time())).await?;
    Ok(consumer.messages().await?)
}

async fn read<J, C>(jetstream: J, clock: C, cache: Arc<Mutex<ReleaseCache>>, mut notices: Notices<J>)
where
    J: InvalidationStreamSource,
    C: EpochClock,
{
    loop {
        drain(&cache, &mut notices).await;
        cache.lock().unwrap_or_else(PoisonError::into_inner).suspend();
        notices = loop {
            tokio::time::sleep(INVALIDATION_RETRY_DELAY).await;
            if let Ok(reading) = subscribe(&jetstream, &clock).await {
                break reading;
            }
        };
        cache.lock().unwrap_or_else(PoisonError::into_inner).resume();
    }
}

/// Drops what each notice covers, until the consumer fails, skips one, or
/// ends.
async fn drain<S, N>(cache: &Mutex<ReleaseCache>, notices: &mut S)
where
    S: Stream<Item = Result<N, pull::MessagesError>> + Unpin,
    N: InvalidationNotice,
{
    let mut expected = 1;
    while let Some(Ok(notice)) = notices.next().await {
        if !matches!(notice.consumer_sequence(), Ok(sequence) if sequence == expected) {
            return;
        }
        let notice = ReleaseInvalidation::from_subject(notice.subject());
        cache.lock().unwrap_or_else(PoisonError::into_inner).invalidate(&notice);
        expected += 1;
    }
}

/// Every tenant's notices from [`INVALIDATION_LOOKBACK`] before `now`, but
/// not before the epoch, as fast as the stream sends them, unacked and kept
/// in memory on one replica, as an ordered consumer is.
fn consumer_config(now: SystemTime) -> pull::Config {
    let start = now
        .checked_sub(INVALIDATION_LOOKBACK)
        .unwrap_or(UNIX_EPOCH)
        .max(UNIX_EPOCH);
    pull::Config {
        filter_subject: INVALIDATION_FILTER.to_owned(),
        deliver_policy: DeliverPolicy::ByStartTime {
            start_time: OffsetDateTime::from(start),
        },
        ack_policy: AckPolicy::None,
        replay_policy: ReplayPolicy::Instant,
        memory_storage: true,
        num_replicas: 1,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
