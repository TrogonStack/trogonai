//! Phase 3 — Exactly-once opt-in on **`PushNotificationConfig`** (idempotency + double-ack).
//!
//! **Status:** compile-only seam — not wired into gateway push dispatch yet. Full design:
//! [`../../../../docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](../../../../docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md).
//!
//! Extension without patching upstream **`a2a-types`**: callers attach [`PushExactlyOnceHints`] beside
//! [`TaskPushNotificationConfig`](https://docs.rs/a2a-types/0.2.0/a2a_types/struct.TaskPushNotificationConfig.html)
//! and negotiate delivery mode via [`PushExactlyOnceNegotiator`]. When hints are absent or
//! [`PushExactlyOnceHints::enable`] is **`false`**, behavior stays **at-least-once** (shipped default).

use a2a_nats::push::PushNotificationTarget;

/// Hypothetical A2A schema boolean extension (`exactlyOnceDelivery`).
pub const EXACTLY_ONCE_DELIVERY_JSON_FIELD: &str = "exactlyOnceDelivery";

/// Hypothetical A2A schema string extension (`idempotencyKeySource`).
pub const IDEMPOTENCY_KEY_SOURCE_JSON_FIELD: &str = "idempotencyKeySource";

/// HTTP cooperative idempotency header for `https://` / `http://` push targets.
pub const HTTP_IDEMPOTENCY_KEY_HEADER: &str = "Idempotency-Key";

/// NATS / JetStream dedupe header for `subject:` and `jetstream:` push targets.
pub const NATS_MSG_ID_HEADER: &str = "Nats-Msg-Id";

/// Typed wire values for [`PushExactlyOnceHints::idempotency_key_source`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdempotencyKeySource {
    /// `{task_id}:{push_config_id}:{terminal_task_state}` — default v1 derivation per sketch.
    TaskTerminalHash,
}

impl IdempotencyKeySource {
    /// Canonical wire string for this derivation strategy.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TaskTerminalHash => "task_terminal_hash",
        }
    }

    /// Parse a wire `idempotencyKeySource` value; unknown strings return **`None`**.
    pub fn parse(wire: &str) -> Option<Self> {
        match wire {
            "task_terminal_hash" => Some(Self::TaskTerminalHash),
            _ => None,
        }
    }
}

/// Hypothetical JSON sidecar for `PushNotificationConfig` until `a2a-types` lands the field.
///
/// Mirrors sketch wire shape:
///
/// ```json
/// {
///   "exactlyOnceDelivery": true,
///   "idempotencyKeySource": "task_terminal_hash"
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushExactlyOnceHints {
    /// When **`false`** or absent on the wire, delivery stays **at-least-once**.
    pub enable: bool,
    /// Agent-side key derivation strategy (wire enum string).
    pub idempotency_key_source: &'static str,
}

impl Default for PushExactlyOnceHints {
    fn default() -> Self {
        Self::disabled()
    }
}

impl PushExactlyOnceHints {
    /// Shipped default — at-least-once; no idempotency headers.
    pub const fn disabled() -> Self {
        Self {
            enable: false,
            idempotency_key_source: IdempotencyKeySource::TaskTerminalHash.as_str(),
        }
    }

    /// Opt-in exactly-once with the v1 `task_terminal_hash` derivation.
    pub const fn enabled_task_terminal_hash() -> Self {
        Self {
            enable: true,
            idempotency_key_source: IdempotencyKeySource::TaskTerminalHash.as_str(),
        }
    }

    /// Build hints from optional wire fields (no serde — gateway ingress parses JSON separately).
    pub fn from_wire_fields(
        exactly_once_delivery: Option<bool>,
        idempotency_key_source: Option<&str>,
    ) -> Self {
        let enable = exactly_once_delivery.unwrap_or(false);
        let source = idempotency_key_source
            .and_then(IdempotencyKeySource::parse)
            .unwrap_or(IdempotencyKeySource::TaskTerminalHash);
        Self {
            enable,
            idempotency_key_source: source.as_str(),
        }
    }

    /// **`true`** when the wire source matches a known [`IdempotencyKeySource`] variant.
    pub fn has_known_key_source(&self) -> bool {
        IdempotencyKeySource::parse(self.idempotency_key_source).is_some()
    }
}

/// Negotiated push delivery semantics for a terminal notification dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactlyOnceDeliveryMode {
    /// Default — in-process retries; duplicates possible; DLQ on terminal failure.
    AtLeastOnce,
    /// Opt-in path — attach transport idempotency headers; per-target retry / ack rules apply.
    ExactlyOncePreferred {
        idempotency_key_header: &'static str,
        /// JetStream publish ack when **`true`**; core NATS `subject:` uses single publish (no retries).
        double_ack: bool,
    },
}

impl ExactlyOnceDeliveryMode {
    pub const fn is_exactly_once_preferred(&self) -> bool {
        matches!(self, Self::ExactlyOncePreferred { .. })
    }
}

/// Extension trait — negotiate delivery mode without mutating upstream `TaskPushNotificationConfig`.
pub trait PushExactlyOnceNegotiator {
    /// Resolved delivery mode for the next terminal push dispatch.
    fn preferred_mode(&self) -> ExactlyOnceDeliveryMode;
}

impl PushExactlyOnceNegotiator for PushExactlyOnceHints {
    fn preferred_mode(&self) -> ExactlyOnceDeliveryMode {
        if !self.enable {
            return ExactlyOnceDeliveryMode::AtLeastOnce;
        }
        // Target-agnostic fallback: HTTP header name until a push URL is available.
        ExactlyOnceDeliveryMode::ExactlyOncePreferred {
            idempotency_key_header: HTTP_IDEMPOTENCY_KEY_HEADER,
            double_ack: false,
        }
    }
}

/// Push config URL paired with exactly-once hints for target-aware negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushConfigExactlyOnceView<'a> {
    /// `TaskPushNotificationConfig.url` (or gateway-side equivalent).
    pub push_url: &'a str,
    pub hints: PushExactlyOnceHints,
}

impl<'a> PushConfigExactlyOnceView<'a> {
    pub const fn new(push_url: &'a str, hints: PushExactlyOnceHints) -> Self {
        Self { push_url, hints }
    }
}

impl PushExactlyOnceNegotiator for PushConfigExactlyOnceView<'_> {
    fn preferred_mode(&self) -> ExactlyOnceDeliveryMode {
        if !self.hints.enable {
            return ExactlyOnceDeliveryMode::AtLeastOnce;
        }

        match PushNotificationTarget::parse(self.push_url) {
            Ok(PushNotificationTarget::Http(_)) => ExactlyOnceDeliveryMode::ExactlyOncePreferred {
                idempotency_key_header: HTTP_IDEMPOTENCY_KEY_HEADER,
                double_ack: false,
            },
            Ok(PushNotificationTarget::JetStream(_)) => ExactlyOnceDeliveryMode::ExactlyOncePreferred {
                idempotency_key_header: NATS_MSG_ID_HEADER,
                double_ack: true,
            },
            Ok(PushNotificationTarget::Nats(_)) => ExactlyOnceDeliveryMode::ExactlyOncePreferred {
                idempotency_key_header: NATS_MSG_ID_HEADER,
                double_ack: false,
            },
            Err(_) => ExactlyOnceDeliveryMode::AtLeastOnce,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_hints_default_to_at_least_once() {
        let hints = PushExactlyOnceHints::disabled();
        assert!(!hints.enable);
        assert_eq!(
            hints.preferred_mode(),
            ExactlyOnceDeliveryMode::AtLeastOnce
        );
    }

    #[test]
    fn enabled_jetstream_target_uses_nats_msg_id_and_double_ack() {
        let view = PushConfigExactlyOnceView::new(
            "jetstream:a2a.push.acme.caller.task",
            PushExactlyOnceHints::enabled_task_terminal_hash(),
        );
        assert_eq!(
            view.preferred_mode(),
            ExactlyOnceDeliveryMode::ExactlyOncePreferred {
                idempotency_key_header: NATS_MSG_ID_HEADER,
                double_ack: true,
            }
        );
    }

    #[test]
    fn enabled_https_target_uses_idempotency_key_header() {
        let view = PushConfigExactlyOnceView::new(
            "https://hooks.example/notify",
            PushExactlyOnceHints::enabled_task_terminal_hash(),
        );
        assert_eq!(
            view.preferred_mode(),
            ExactlyOnceDeliveryMode::ExactlyOncePreferred {
                idempotency_key_header: HTTP_IDEMPOTENCY_KEY_HEADER,
                double_ack: false,
            }
        );
    }

    #[test]
    fn enabled_subject_target_single_publish_best_effort() {
        let view = PushConfigExactlyOnceView::new(
            "subject:a2a.push.notify",
            PushExactlyOnceHints::enabled_task_terminal_hash(),
        );
        assert_eq!(
            view.preferred_mode(),
            ExactlyOnceDeliveryMode::ExactlyOncePreferred {
                idempotency_key_header: NATS_MSG_ID_HEADER,
                double_ack: false,
            }
        );
    }

    #[test]
    fn wire_fields_absent_stay_at_least_once() {
        let hints = PushExactlyOnceHints::from_wire_fields(None, None);
        assert!(!hints.enable);
        assert_eq!(
            hints.preferred_mode(),
            ExactlyOnceDeliveryMode::AtLeastOnce
        );
    }

    #[test]
    fn idempotency_key_source_round_trips_task_terminal_hash() {
        assert_eq!(
            IdempotencyKeySource::TaskTerminalHash.as_str(),
            "task_terminal_hash"
        );
        assert_eq!(
            IdempotencyKeySource::parse("task_terminal_hash"),
            Some(IdempotencyKeySource::TaskTerminalHash)
        );
    }
}
