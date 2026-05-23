//! §5 roadmap — Gateway pull consumer for task-event egress (`A2A_EVENTS`) with JetStream flow control.
//!
//! **Status:** not wired into ingress runtime. Full operator and implementer guide:
//! [`../../../../docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](../../../../docs/A2A_STREAMING_BACKPRESSURE_OPS.md).
//!
//! ## §5 recap (landed decision)
//!
//! Agents publish `TaskStatusUpdateEvent` / `TaskArtifactUpdateEvent` JSON to per-task JetStream
//! subjects on the shared Account stream **`A2A_EVENTS`**. Slow or crashed downstream readers must
//! **never** block agent publish — that violates the A2A contract that agents keep working
//! independently of client read speed.
//!
//! The landed pairing for stream policy is **`retention=interest`** + **`discard=old`**: when pressure
//! hits, oldest events drop inside the retention window instead of blocking publishers. Gateway egress
//! adds a second layer: an **ephemeral pull consumer** with **flow control** and a bounded
//! **`max_ack_pending`** so slow caller-side consumption throttles **fetch**, not agent **publish**.
//!
//! Target gateway pipe (Phase 2):
//!
//! 1. **`message/stream`** — forward RPC to agent, then subscribe via ephemeral pull consumer on
//!    `{prefix}.task.{task_id}.events.*` (or req-scoped filter), rewrite/redact, deliver to caller inbox.
//! 2. **`tasks/resubscribe`** — ephemeral consumer from **`last_seq + 1`** (same semantics as
//!    in-tree `resubscribe_consumer`).
//! 3. **Explicit ack** after successful forward to caller inbox; **`inactive_threshold = 5m`** for
//!    consumer hygiene (matches shipped client path).
//!
//! Agent ingress back-pressure (`Bridge` semaphore / `A2A_MAX_CONCURRENT_CLIENT_TASKS`) remains a
//! separate concern — it caps concurrent RPC handlers, not JetStream consumer flow control.

use std::time::Duration;

use a2a_nats::{A2aPrefix, A2aTaskId, ReqId};
use async_nats::jetstream::consumer::AckPolicy;

/// Default **`max_ack_pending`** for gateway task-event egress (order of tens, not thousands).
///
/// Tune to gateway rewrite/forward worker pool capacity — see ops guide §Gateway egress.
pub const DEFAULT_MAX_ACK_PENDING: usize = 32;

/// Default **`inactive_threshold`** for ephemeral gateway consumers (matches in-tree client path).
pub const DEFAULT_INACTIVE_THRESHOLD_SECS: u64 = 300;

/// JetStream pull consumer tuning for gateway **`A2A_EVENTS`** egress.
///
/// Gateway consumers should enable **flow control** (not represented here — set on subscribe /
/// consumer config at wiring time) and reuse the same ack / hygiene discipline as
/// `a2a_nats::jetstream::consumers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullConsumerHints {
    /// Bounded in-flight unacked messages — matches gateway rewrite/forward capacity.
    pub max_ack_pending: usize,
    /// Ephemeral consumer expiry when idle; prevents metadata leaks after caller disconnect.
    pub inactive_threshold_secs: u64,
    /// Ack policy for the pull consumer. Gateway egress uses **explicit** ack after successful
    /// forward to the caller inbox (never auto-ack before rewrite/redact completes).
    pub ack_policy: AckPolicy,
}

impl PullConsumerHints {
    /// Operator baseline from landed §5 / [`A2A_STREAMING_BACKPRESSURE_OPS.md`](../../../../docs/A2A_STREAMING_BACKPRESSURE_OPS.md).
    pub const fn gateway_baseline() -> Self {
        Self {
            max_ack_pending: DEFAULT_MAX_ACK_PENDING,
            inactive_threshold_secs: DEFAULT_INACTIVE_THRESHOLD_SECS,
            ack_policy: AckPolicy::Explicit,
        }
    }

    pub fn inactive_threshold(&self) -> Duration {
        Duration::from_secs(self.inactive_threshold_secs)
    }
}

impl Default for PullConsumerHints {
    fn default() -> Self {
        Self::gateway_baseline()
    }
}

/// How a gateway forward loop should disposition a JetStream message after a delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressAckDisposition {
    /// Forward succeeded — **`Ack`** the JetStream message.
    Ack,
    /// Transient rewrite / inbox publish failure — **`Nak`** for redelivery (optional delay).
    Nak {
        /// Redelivery delay; `None` uses server default.
        delay: Option<Duration>,
    },
    /// Poison or policy reject — **`Term`** so the message is not redelivered indefinitely.
    Term,
}

/// Planned subscribe shape for **`tasks/resubscribe`** gateway egress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResubscribeEgressPlan {
    pub filter_subject: String,
    pub start_sequence: u64,
}

/// Planned subscribe shape for initial **`message/stream`** gateway egress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageStreamEgressPlan {
    pub filter_subject: String,
    pub hints: PullConsumerHints,
}

/// Seams for future gateway-owned JetStream pull subscribe + Nak/Term strategy on **`A2A_EVENTS`**.
///
/// Production wiring will create ephemeral consumers on `A2A_EVENTS`, enable flow control on
/// subscribe, fetch in a bounded loop, rewrite/redact, forward to caller inbox, then apply
/// [`EgressAckDisposition`] per message.
///
/// # Example — gateway pull egress loop (pseudocode; not wired)
///
/// ```ignore
/// use a2a_gateway::planned::gw_pull_backpressure::{
///     BaselineTaskEventsEgressPlanner, EgressAckDisposition, TaskEventsEgressPlanner,
/// };
///
/// let planner = BaselineTaskEventsEgressPlanner::new();
/// let hints = planner.pull_hints();
/// let plan = planner.plan_message_stream(&prefix, &req_id)?;
///
/// // Ephemeral pull consumer on A2A_EVENTS (illustrative):
/// //   filter_subject:  plan.filter_subject
/// //   deliver_policy:  all | by_start_sequence (resubscribe)
/// //   ack_policy:      hints.ack_policy  // Explicit
/// //   replay_policy:   instant
/// //   flow_control:    true
/// //   max_ack_pending: hints.max_ack_pending
/// //   inactive_threshold: hints.inactive_threshold()
///
/// let mut attempt = 0u32;
/// loop {
///     let Some(msg) = consumer.fetch(1).await? else { continue };
///     attempt += 1;
///     let forward_err = rewrite_and_publish_to_caller_inbox(&msg).err();
///     match planner.forward_disposition(attempt, forward_err.as_deref()) {
///         EgressAckDisposition::Ack => msg.ack().await?,
///         EgressAckDisposition::Nak { delay } => msg.nak_with_delay(delay).await?,
///         EgressAckDisposition::Term => msg.term().await?,
///     }
///     if matches!(forward_err, None) {
///         attempt = 0;
///     }
/// }
/// ```
pub trait TaskEventsEgressPlanner {
    type Error: std::error::Error + Send + Sync + 'static;

    fn pull_hints(&self) -> PullConsumerHints;

    /// Ephemeral pull consumer plan for `message/stream` after agent bootstrap reply.
    fn plan_message_stream(
        &self,
        prefix: &A2aPrefix,
        req_id: &ReqId,
    ) -> Result<MessageStreamEgressPlan, Self::Error>;

    /// Ephemeral pull consumer plan for `tasks/resubscribe` replay from `last_seq + 1`.
    fn plan_resubscribe(
        &self,
        prefix: &A2aPrefix,
        task_id: &A2aTaskId,
        last_seq: u64,
    ) -> Result<ResubscribeEgressPlan, Self::Error>;

    /// Nak/Term vs Ack after attempting to forward one task event to the caller inbox.
    fn forward_disposition(
        &self,
        attempt: u32,
        forward_error: Option<&str>,
    ) -> EgressAckDisposition;
}

/// Compile-only baseline planner — documents intended filters and ack discipline until runtime wiring.
#[derive(Debug, Default, Clone, Copy)]
pub struct BaselineTaskEventsEgressPlanner {
    hints: PullConsumerHints,
}

impl BaselineTaskEventsEgressPlanner {
    pub const fn new() -> Self {
        Self {
            hints: PullConsumerHints::gateway_baseline(),
        }
    }
}

impl TaskEventsEgressPlanner for BaselineTaskEventsEgressPlanner {
    type Error = std::convert::Infallible;

    fn pull_hints(&self) -> PullConsumerHints {
        self.hints
    }

    fn plan_message_stream(
        &self,
        prefix: &A2aPrefix,
        req_id: &ReqId,
    ) -> Result<MessageStreamEgressPlan, Self::Error> {
        Ok(MessageStreamEgressPlan {
            filter_subject: format!("{}.task.*.events.{req_id}", prefix.as_str()),
            hints: self.hints,
        })
    }

    fn plan_resubscribe(
        &self,
        prefix: &A2aPrefix,
        task_id: &A2aTaskId,
        last_seq: u64,
    ) -> Result<ResubscribeEgressPlan, Self::Error> {
        Ok(ResubscribeEgressPlan {
            filter_subject: format!("{}.task.{task_id}.events.*", prefix.as_str()),
            start_sequence: last_seq.saturating_add(1),
        })
    }

    fn forward_disposition(
        &self,
        attempt: u32,
        forward_error: Option<&str>,
    ) -> EgressAckDisposition {
        match forward_error {
            None => EgressAckDisposition::Ack,
            Some(_) if attempt < 3 => EgressAckDisposition::Nak { delay: None },
            Some(_) => EgressAckDisposition::Term,
        }
    }
}
