use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy, pull};
use futures::{Stream, StreamExt};
use time::OffsetDateTime;
use tokio::sync::oneshot;
use tokio::task::AbortHandle;
use tokio::time::Instant;
use trogon_std::time::{EpochClock, GetElapsed};

use super::constants::{
    INVALIDATION_FILTER, INVALIDATION_LOOKBACK, INVALIDATION_POSITION_CHECK, INVALIDATION_RETRY_DELAY,
    INVALIDATION_STREAM,
};
use super::telemetry::ReleaseCacheMetrics;
use super::{
    InvalidationConsumer, InvalidationLoss, InvalidationNotice, InvalidationStartError, InvalidationStream,
    InvalidationStreamError, InvalidationStreamSource, ReleaseCache, ReleaseInvalidation,
};

type Consumer<J> = <<J as InvalidationStreamSource>::Stream as InvalidationStream>::Consumer;
type Notices<J> = <Consumer<J> as InvalidationConsumer>::Notices;
type Reading<J> = (Consumer<J>, Notices<J>, u64);

/// One client's own reader of release invalidations, read the way an ordered
/// consumer reads: an ephemeral consumer of its own, never durable and never
/// in a queue group, so every replica hears every notice (ADR#0023 Decision
/// 6), with no acks, checked to arrive one after another.
///
/// It starts [`INVALIDATION_LOOKBACK`] back, so a notice sent while it was
/// not reading is still heard. Those notices are already in the stream when
/// its consumer is created, and each one it replays drops what it covers,
/// so the reader is not caught up, and holds nothing, until it has received
/// every one the stream held at that moment, or a quiet
/// [`INVALIDATION_POSITION_CHECK`] finds the server owes it nothing more. Starting waits for that, so a
/// release asked for after it is not overtaken by the replay, and so does
/// every return to reading. It loses its place on any error, any gap in
/// what it was delivered, the end of its consumer, or a notice the server
/// says it delivered that has not arrived by the next
/// [`INVALIDATION_POSITION_CHECK`]; then it drops every
/// held copy and holds none, retrying every [`INVALIDATION_RETRY_DELAY`]
/// until it reads again and has caught up. It stops when dropped.
///
/// Each loss of place is counted under why it happened and warned of once,
/// however long the reader takes to read again; reading again is logged
/// with how many attempts it took.
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
    /// Returns once the first consumer has caught up with the notices the
    /// stream held when it was created, or once the reader has lost its place
    /// before that, when nothing is held and it keeps retrying. Giving up on
    /// it before then stops the reader.
    pub(crate) async fn start<J, C>(
        jetstream: J,
        clock: C,
        cache: Arc<Mutex<ReleaseCache<C>>>,
        metrics: ReleaseCacheMetrics,
    ) -> Result<Self, InvalidationStartError>
    where
        J: InvalidationStreamSource,
        C: EpochClock + GetElapsed,
        C::Instant: Send,
    {
        let reading = subscribe(&jetstream, &clock).await?;
        let (caught_up, ready) = oneshot::channel();
        let reader = Self(tokio::spawn(read(jetstream, clock, cache, metrics, reading, caught_up)).abort_handle());
        let _ = ready.await;
        Ok(reader)
    }
}

impl Drop for InvalidationReader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn subscribe<J, C>(jetstream: &J, clock: &C) -> Result<Reading<J>, InvalidationStartError>
where
    J: InvalidationStreamSource,
    C: EpochClock,
{
    let stream = jetstream.get_stream(INVALIDATION_STREAM).await?;
    InvalidationStreamError::check(&stream.config())?;
    let consumer = stream.create_consumer(consumer_config(clock.system_time())).await?;
    let backlog = consumer.backlog();
    let notices = consumer.messages().await?;
    Ok((consumer, notices, backlog))
}

async fn read<J, C>(
    jetstream: J,
    clock: C,
    cache: Arc<Mutex<ReleaseCache<C>>>,
    metrics: ReleaseCacheMetrics,
    mut reading: Reading<J>,
    started: oneshot::Sender<()>,
) where
    J: InvalidationStreamSource,
    C: EpochClock + GetElapsed,
    C::Instant: Send,
{
    let mut started = Some(started);
    let mut attempts: u64 = 0;
    loop {
        let (consumer, notices, backlog) = &mut reading;
        let loss = drain(&cache, consumer, notices, *backlog, || match started.take() {
            Some(started) => {
                let _ = started.send(());
            }
            None => {
                cache.lock().unwrap_or_else(PoisonError::into_inner).resume();
                tracing::info!(attempts, "invalidation reader reads again; releases are held again");
            }
        })
        .await;
        cache.lock().unwrap_or_else(PoisonError::into_inner).suspend();
        started = None;
        metrics.record_position_loss(&loss);
        let reason = loss.reason().as_str();
        tracing::warn!(
            reason,
            detail = %loss,
            "invalidation reader lost its place; releases are not held until it reads again"
        );
        attempts = 0;
        reading = loop {
            tokio::time::sleep(INVALIDATION_RETRY_DELAY).await;
            attempts += 1;
            if let Ok(reading) = subscribe(&jetstream, &clock).await {
                break reading;
            }
        };
    }
}

/// Drops what each notice covers, until the consumer fails, skips one, ends,
/// or delivered one that never arrived, and says which.
///
/// Calls `caught_up` once, when `backlog` notices have been received, or at
/// once when there are none, or when a look after a quiet interval finds the
/// server owes it nothing, since the backlog was then counted too high and
/// the replay is over. A drain that ends before then never calls it.
///
/// Every [`INVALIDATION_POSITION_CHECK`] with no notice since the last look,
/// it asks the server how far `consumer` has delivered. A notice delivered
/// past the last one received may still be on its way, so it is owed until
/// the next look; still missing then, the reader has lost its place. Without
/// this, a lost last notice would show only when a later one arrived.
async fn drain<K, S, N, C>(
    cache: &Mutex<ReleaseCache<C>>,
    consumer: &K,
    notices: &mut S,
    backlog: u64,
    caught_up: impl FnOnce(),
) -> InvalidationLoss
where
    K: InvalidationConsumer,
    S: Stream<Item = Result<N, pull::MessagesError>> + Unpin,
    N: InvalidationNotice,
    C: GetElapsed,
{
    let mut caught_up = Some(caught_up);
    let mut received = 0;
    if backlog == 0 {
        call_once(&mut caught_up);
    }
    let mut arrived = false;
    let mut owed = None;
    let mut next_look = Instant::now() + INVALIDATION_POSITION_CHECK;
    loop {
        match tokio::time::timeout_at(next_look, notices.next()).await {
            Ok(Some(Ok(notice))) => {
                let expected = received + 1;
                match notice.consumer_sequence() {
                    Ok(sequence) if sequence == expected => {}
                    Ok(arrived) => return InvalidationLoss::Gap { expected, arrived },
                    Err(unplaced) => return InvalidationLoss::Unplaced(unplaced),
                }
                let notice = ReleaseInvalidation::from_subject(notice.subject());
                cache.lock().unwrap_or_else(PoisonError::into_inner).invalidate(&notice);
                received += 1;
                arrived = true;
                if received >= backlog {
                    call_once(&mut caught_up);
                }
            }
            Ok(Some(Err(failed))) => return InvalidationLoss::Failed(failed),
            Ok(None) => return InvalidationLoss::Ended,
            Err(_) => {
                if let Some(delivered) = owed.take().filter(|&delivered| received < delivered) {
                    return InvalidationLoss::Missing { received, delivered };
                }
                if !std::mem::take(&mut arrived) {
                    let delivered = match consumer.delivered().await {
                        Ok(delivered) => delivered,
                        Err(unknown) => return InvalidationLoss::PositionUnknown(unknown),
                    };
                    owed = (delivered > received).then_some(delivered);
                    if owed.is_none() {
                        call_once(&mut caught_up);
                    }
                }
                next_look = Instant::now() + INVALIDATION_POSITION_CHECK;
            }
        }
    }
}

fn call_once(call: &mut Option<impl FnOnce()>) {
    if let Some(call) = call.take() {
        call();
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
