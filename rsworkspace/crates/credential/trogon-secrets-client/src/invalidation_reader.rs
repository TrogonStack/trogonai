use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull};
use futures::{Stream, StreamExt};
use time::OffsetDateTime;
use tokio::task::AbortHandle;
use tokio::time::Instant;
use trogon_std::time::{EpochClock, GetElapsed};

use super::constants::{
    INVALIDATION_FILTER, INVALIDATION_LOOKBACK, INVALIDATION_POSITION_CHECK, INVALIDATION_RETRY_DELAY,
    INVALIDATION_STREAM,
};
use super::{
    InvalidationConsumer, InvalidationNotice, InvalidationStartError, InvalidationStream, InvalidationStreamError,
    InvalidationStreamSource, ReleaseCache, ReleaseInvalidation,
};

type Consumer<J> = <<J as InvalidationStreamSource>::Stream as InvalidationStream>::Consumer;
type Notices<J> = <Consumer<J> as InvalidationConsumer>::Notices;

/// One client's own reader of release invalidations, read the way an ordered
/// consumer reads: an ephemeral consumer of its own, never durable and never
/// in a queue group, so every replica hears every notice (ADR#0023 Decision
/// 6), with no acks, checked to arrive one after another.
///
/// It starts [`INVALIDATION_LOOKBACK`] back, so a notice sent while it was
/// not reading is still heard. It loses its place on any error, any gap in
/// what it was delivered, the end of its consumer, or a notice the server
/// says it delivered that has not arrived by the next
/// [`INVALIDATION_POSITION_CHECK`]; then it drops every
/// held copy and holds none, retrying every [`INVALIDATION_RETRY_DELAY`]
/// until it reads again. It stops when dropped.
///
/// It never looks at stream sequences. A notice the stream removed before
/// delivering it would leave the consumer sequences unbroken, so the reader
/// refuses, at every subscribe, a stream configured to remove or rewrite a
/// notice inside the lookback ([`InvalidationStreamError`]); within one it
/// accepts, no notice leaves the stream there, and consumer sequences are
/// enough. A refused stream stops the client from starting, and later keeps
/// the reader retrying with nothing held.
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
        cache: Arc<Mutex<ReleaseCache<C>>>,
    ) -> Result<Self, InvalidationStartError>
    where
        J: InvalidationStreamSource,
        C: EpochClock + GetElapsed,
        C::Instant: Send,
    {
        let reading = subscribe(&jetstream, &clock).await?;
        let task = tokio::spawn(read(jetstream, clock, cache, reading));
        Ok(Self(task.abort_handle()))
    }
}

impl Drop for InvalidationReader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn subscribe<J, C>(jetstream: &J, clock: &C) -> Result<(Consumer<J>, Notices<J>), InvalidationStartError>
where
    J: InvalidationStreamSource,
    C: EpochClock,
{
    let stream = jetstream.get_stream(INVALIDATION_STREAM).await?;
    InvalidationStreamError::check(&stream.config())?;
    let consumer = stream.create_consumer(consumer_config(clock.system_time())).await?;
    let notices = consumer.messages().await?;
    Ok((consumer, notices))
}

async fn read<J, C>(jetstream: J, clock: C, cache: Arc<Mutex<ReleaseCache<C>>>, mut reading: (Consumer<J>, Notices<J>))
where
    J: InvalidationStreamSource,
    C: EpochClock + GetElapsed,
    C::Instant: Send,
{
    loop {
        let (consumer, notices) = &mut reading;
        drain(&cache, consumer, notices).await;
        cache.lock().unwrap_or_else(PoisonError::into_inner).suspend();
        reading = loop {
            tokio::time::sleep(INVALIDATION_RETRY_DELAY).await;
            if let Ok(reading) = subscribe(&jetstream, &clock).await {
                break reading;
            }
        };
        cache.lock().unwrap_or_else(PoisonError::into_inner).resume();
    }
}

/// Drops what each notice covers, until the consumer fails, skips one, ends,
/// or delivered one that never arrived.
///
/// Every [`INVALIDATION_POSITION_CHECK`] with no notice since the last look,
/// it asks the server how far `consumer` has delivered. A notice delivered
/// past the last one received may still be on its way, so it is owed until
/// the next look; still missing then, the reader has lost its place. Without
/// this, a lost last notice would show only when a later one arrived.
async fn drain<K, S, N, C>(cache: &Mutex<ReleaseCache<C>>, consumer: &K, notices: &mut S)
where
    K: InvalidationConsumer,
    S: Stream<Item = Result<N, pull::MessagesError>> + Unpin,
    N: InvalidationNotice,
    C: GetElapsed,
{
    let mut received = 0;
    let mut arrived = false;
    let mut owed = None;
    let mut next_look = Instant::now() + INVALIDATION_POSITION_CHECK;
    loop {
        match tokio::time::timeout_at(next_look, notices.next()).await {
            Ok(Some(Ok(notice))) => {
                if !matches!(notice.consumer_sequence(), Ok(sequence) if sequence == received + 1) {
                    return;
                }
                let notice = ReleaseInvalidation::from_subject(notice.subject());
                cache.lock().unwrap_or_else(PoisonError::into_inner).invalidate(&notice);
                received += 1;
                arrived = true;
            }
            Ok(_) => return,
            Err(_) => {
                if owed.take().is_some_and(|owed| received < owed) {
                    return;
                }
                if !std::mem::take(&mut arrived) {
                    let Ok(delivered) = consumer.delivered().await else {
                        return;
                    };
                    owed = (delivered > received).then_some(delivered);
                }
                next_look = Instant::now() + INVALIDATION_POSITION_CHECK;
            }
        }
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
