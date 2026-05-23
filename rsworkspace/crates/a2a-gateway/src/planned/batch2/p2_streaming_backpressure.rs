//! Phase 2 — Gateway pull consumer + flow control; **`A2A_EVENTS`** interest + discard-old.
//!
//! **Status:** compile-only seam — not wired into [`crate::runtime`]. Full operator and
//! implementer guide:
//! [`../../../../docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](../../../../docs/A2A_STREAMING_BACKPRESSURE_OPS.md).
//!
//! ## Operator delta vs in-tree provisioning
//!
//! In-tree [`provision_streams`](a2a_nats::jetstream::provision) / [`A2aStream::Events`]
//! (`a2a-nats/src/nats/subjects/stream.rs`) already sets **`discard=old`** but **`retention=limits`**.
//! Phase 2 requires **`retention=interest`** + **`discard=old`** on the Account stream **`A2A_EVENTS`**
//! so agents never block on publish when downstream consumers lag or disconnect.
//!
//! | Setting | In-tree `provision_streams` today | Phase 2 target |
//! |---------|-------------------------------------|----------------|
//! | **`retention`** | **`limits`** | **`interest`** |
//! | **`discard`** | **`old`** | **`old`** (unchanged) |
//! | **`max_age`** | **`24h`** baseline | **`24h`** baseline (operator may extend) |
//!
//! Operators provisioning outside the helper (CLI, Terraform, platform tooling) should apply the
//! Phase 2 pair now; see [`ADVISORY_JETSTREAM_REPROVISION_NOTE`] and the ops guide §Stream policy.

use std::time::Duration;

use a2a_nats::nats::subjects::A2aStream;
use a2a_nats::{A2aPrefix, A2aTaskId, ReqId};
use async_nats::jetstream::consumer::AckPolicy;
use async_nats::jetstream::stream::{DiscardPolicy, RetentionPolicy};

/// Operator advisory when **`A2A_EVENTS`** was provisioned with in-tree **`limits`** retention.
///
/// Reprovision (or patch via server-side tooling) to **`retention=interest`**, **`discard=old`**
/// before relying on gateway egress flow control for publish-side back-pressure guarantees.
pub const ADVISORY_JETSTREAM_REPROVISION_NOTE: &str = "\
A2A_EVENTS was likely provisioned by in-tree provision_streams with retention=limits and \
discard=old. Phase 2 requires retention=interest with discard=old so unconsumed backlog can \
reclaim when consumer interest ends and oldest events drop under pressure instead of blocking \
agent publish. Reprovision the stream (nats stream edit / platform IaC) before enabling gateway \
task-event egress. See docs/A2A_STREAMING_BACKPRESSURE_OPS.md.";

/// Target JetStream stream policy for gateway task-event egress.
///
/// When [`Self::discard_old_interest_required`] is **`true`**, the Account **`A2A_EVENTS`** stream
/// must run **`retention=interest`** + **`discard=old`** — not the in-tree **`limits`** default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPolicyTarget {
    /// **`true`** when the stream must use **`interest`** retention and **`discard=old`**.
    pub discard_old_interest_required: bool,
}

impl StreamPolicyTarget {
    /// Landed Phase 2 policy for **`A2A_EVENTS`** ([`A2A_STREAMING_BACKPRESSURE_OPS.md`](../../../../docs/A2A_STREAMING_BACKPRESSURE_OPS.md)).
    pub const fn a2a_events() -> Self {
        Self {
            discard_old_interest_required: true,
        }
    }

    /// Expected retention when [`Self::discard_old_interest_required`] is set.
    pub const fn retention(&self) -> RetentionPolicy {
        if self.discard_old_interest_required {
            RetentionPolicy::Interest
        } else {
            RetentionPolicy::Limits
        }
    }

    /// Expected discard policy (always **`old`** for task events when interest is required).
    pub const fn discard(&self) -> DiscardPolicy {
        DiscardPolicy::Old
    }

    /// **`true`** when in-tree [`A2aStream::Events`] provisioning matches this target.
    pub fn matches_in_tree_provision(&self, prefix: &A2aPrefix) -> bool {
        let provisioned = A2aStream::Events.config(prefix);
        provisioned.retention == self.retention() && provisioned.discard == self.discard()
    }

    /// **`true`** when operators must reprovision **`A2A_EVENTS`** before gateway egress ships.
    pub fn requires_operator_reprovision(&self, prefix: &A2aPrefix) -> bool {
        self.discard_old_interest_required && !self.matches_in_tree_provision(prefix)
    }
}

/// Default **`max_ack_pending`** for gateway task-event egress (order of tens, not thousands).
pub const DEFAULT_MAX_ACK_PENDING: usize = 32;

/// Default **`inactive_threshold`** for ephemeral gateway consumers (matches in-tree client path).
pub const DEFAULT_INACTIVE_THRESHOLD_SECS: u64 = 300;

/// Gateway pull consumer tuning on **`A2A_EVENTS`** — flow control enabled at wiring time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayPullConsumerHints {
    /// JetStream consumer flow control — server stops pushing batches when fetch loop falls behind.
    pub flow_control: bool,
    /// Bounded in-flight unacked messages — matches gateway rewrite/forward capacity.
    pub max_ack_pending: usize,
    /// Ephemeral consumer expiry when idle; prevents metadata leaks after caller disconnect.
    pub inactive_threshold_secs: u64,
    /// Explicit ack after successful forward to caller inbox.
    pub ack_policy: AckPolicy,
}

impl GatewayPullConsumerHints {
    /// Operator baseline from landed §5 / ops guide §Gateway egress.
    pub const fn gateway_baseline() -> Self {
        Self {
            flow_control: true,
            max_ack_pending: DEFAULT_MAX_ACK_PENDING,
            inactive_threshold_secs: DEFAULT_INACTIVE_THRESHOLD_SECS,
            ack_policy: AckPolicy::Explicit,
        }
    }

    pub fn inactive_threshold(&self) -> Duration {
        Duration::from_secs(self.inactive_threshold_secs)
    }
}

impl Default for GatewayPullConsumerHints {
    fn default() -> Self {
        Self::gateway_baseline()
    }
}

/// Planned ephemeral pull consumer for initial **`message/stream`** gateway egress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageStreamPullPlan {
    pub stream_name: String,
    pub filter_subject: String,
    pub hints: GatewayPullConsumerHints,
}

/// Planned ephemeral pull consumer for **`tasks/resubscribe`** gateway egress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResubscribePullPlan {
    pub stream_name: String,
    pub filter_subject: String,
    pub start_sequence: u64,
    pub hints: GatewayPullConsumerHints,
}

/// Compile-only factory seam for gateway-owned JetStream pull consumers on task events.
///
/// Production wiring creates ephemeral consumers on **`A2A_EVENTS`**, enables flow control,
/// fetches in a bounded loop, rewrite/redacts, forwards to the caller inbox, then acks explicitly.
/// No `async_nats` JetStream client trait impl lives here — stubs only until runtime wiring.
pub trait GatewayTaskEventPullConsumerFactory {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Stream policy the gateway expects before creating egress consumers.
    fn stream_policy_target(&self) -> StreamPolicyTarget;

    /// Pull consumer hints shared by **`message/stream`** and **`tasks/resubscribe`** egress.
    fn pull_consumer_hints(&self) -> GatewayPullConsumerHints;

    /// Ephemeral pull consumer plan for **`message/stream`** after agent bootstrap reply.
    fn plan_message_stream(
        &self,
        prefix: &A2aPrefix,
        req_id: &ReqId,
    ) -> Result<MessageStreamPullPlan, Self::Error>;

    /// Ephemeral pull consumer plan for **`tasks/resubscribe`** replay from **`last_seq + 1`**.
    fn plan_resubscribe(
        &self,
        prefix: &A2aPrefix,
        task_id: &A2aTaskId,
        last_seq: u64,
    ) -> Result<ResubscribePullPlan, Self::Error>;
}

/// Baseline compile-only factory — documents intended filters, flow control, and stream policy.
#[derive(Debug, Clone, Copy)]
pub struct BaselineGatewayTaskEventPullConsumerFactory {
    policy: StreamPolicyTarget,
    hints: GatewayPullConsumerHints,
}

impl BaselineGatewayTaskEventPullConsumerFactory {
    pub const fn new() -> Self {
        Self {
            policy: StreamPolicyTarget::a2a_events(),
            hints: GatewayPullConsumerHints::gateway_baseline(),
        }
    }
}

impl Default for BaselineGatewayTaskEventPullConsumerFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl GatewayTaskEventPullConsumerFactory for BaselineGatewayTaskEventPullConsumerFactory {
    type Error = std::convert::Infallible;

    fn stream_policy_target(&self) -> StreamPolicyTarget {
        self.policy
    }

    fn pull_consumer_hints(&self) -> GatewayPullConsumerHints {
        self.hints
    }

    fn plan_message_stream(
        &self,
        prefix: &A2aPrefix,
        req_id: &ReqId,
    ) -> Result<MessageStreamPullPlan, Self::Error> {
        Ok(MessageStreamPullPlan {
            stream_name: A2aStream::Events.stream_name(prefix),
            filter_subject: format!("{}.task.*.events.{req_id}", prefix.as_str()),
            hints: self.hints,
        })
    }

    fn plan_resubscribe(
        &self,
        prefix: &A2aPrefix,
        task_id: &A2aTaskId,
        last_seq: u64,
    ) -> Result<ResubscribePullPlan, Self::Error> {
        Ok(ResubscribePullPlan {
            stream_name: A2aStream::Events.stream_name(prefix),
            filter_subject: format!("{}.task.{task_id}.events.*", prefix.as_str()),
            start_sequence: last_seq.saturating_add(1),
            hints: self.hints,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> A2aPrefix {
        A2aPrefix::new("a2a".to_string()).expect("test prefix")
    }

    fn task_id(s: &str) -> A2aTaskId {
        A2aTaskId::new(s).expect("test task id")
    }

    fn req_id(s: &str) -> ReqId {
        ReqId::from_header(s)
    }

    #[test]
    fn stream_policy_target_differs_from_in_tree_provision() {
        let target = StreamPolicyTarget::a2a_events();
        let prefix = prefix();
        assert!(target.discard_old_interest_required);
        assert_eq!(target.retention(), RetentionPolicy::Interest);
        assert_eq!(target.discard(), DiscardPolicy::Old);
        assert!(!target.matches_in_tree_provision(&prefix));
        assert!(target.requires_operator_reprovision(&prefix));
    }

    #[test]
    fn baseline_factory_plans_match_client_path_filters() {
        let factory = BaselineGatewayTaskEventPullConsumerFactory::new();
        let prefix = prefix();
        let stream = factory
            .plan_message_stream(&prefix, &req_id("r1"))
            .expect("message/stream plan");
        assert_eq!(stream.stream_name, "A2A_EVENTS");
        assert_eq!(stream.filter_subject, "a2a.task.*.events.r1");
        assert!(stream.hints.flow_control);

        let resub = factory
            .plan_resubscribe(&prefix, &task_id("t1"), 42)
            .expect("resubscribe plan");
        assert_eq!(resub.filter_subject, "a2a.task.t1.events.*");
        assert_eq!(resub.start_sequence, 43);
    }

    #[test]
    fn advisory_note_mentions_interest_retention() {
        assert!(ADVISORY_JETSTREAM_REPROVISION_NOTE.contains("retention=interest"));
        assert!(ADVISORY_JETSTREAM_REPROVISION_NOTE.contains("retention=limits"));
    }
}
