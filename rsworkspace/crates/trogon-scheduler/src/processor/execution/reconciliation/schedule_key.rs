use uuid::Uuid;

use crate::commands::domain::ScheduleId;

const SCHEDULER_SCHEDULE_NAMESPACE: Uuid = Uuid::from_u128(0x1f8e_7d6c_5b4a_4938_8271_6050_4f3e_2d1c);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScheduleKey(Uuid);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamRoutingId(String);

impl StreamRoutingId {
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for StreamRoutingId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for StreamRoutingId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl ScheduleKey {
    pub fn derive(schedule_id: &ScheduleId) -> Self {
        Self(Uuid::new_v5(
            &SCHEDULER_SCHEDULE_NAMESPACE,
            schedule_id.as_str().as_bytes(),
        ))
    }

    /// Derives the key directly from a raw stream id for lane routing.
    ///
    /// For any valid [`ScheduleId`] this equals [`derive`](Self::derive); it is
    /// also total over arbitrary stream id strings so the lane dispatcher can
    /// route every delivered record to a deterministic aggregate lane.
    pub fn for_stream(stream_id: &StreamRoutingId) -> Self {
        Self(Uuid::new_v5(&SCHEDULER_SCHEDULE_NAMESPACE, stream_id.as_bytes()))
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    pub fn simple(&self) -> String {
        self.0.as_simple().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule_id(raw: &str) -> ScheduleId {
        ScheduleId::parse(raw).unwrap()
    }

    #[test]
    fn derivation_is_deterministic_for_the_same_schedule_id() {
        let first = ScheduleKey::derive(&schedule_id("orders/created"));
        let second = ScheduleKey::derive(&schedule_id("orders/created"));

        assert_eq!(first, second);
        assert_eq!(first.as_uuid(), second.as_uuid());
    }

    #[test]
    fn different_schedule_ids_derive_different_keys() {
        let first = ScheduleKey::derive(&schedule_id("orders/created"));
        let second = ScheduleKey::derive(&schedule_id("orders/updated"));

        assert_ne!(first, second);
    }

    #[test]
    fn simple_representation_is_thirty_two_lowercase_hex_digits() {
        let key = ScheduleKey::derive(&schedule_id("日次バックアップ"));
        let simple = key.simple();

        assert_eq!(simple.len(), 32);
        assert!(
            simple
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
        );
    }

    #[test]
    fn non_nats_token_schedule_ids_still_derive_a_token_safe_key() {
        for raw in ["report.v2", "orders/created", "ns:thing", "user@host", "café-nightly"] {
            let simple = ScheduleKey::derive(&schedule_id(raw)).simple();
            assert_eq!(simple.len(), 32, "{raw}");
        }
    }

    #[test]
    fn stream_routing_id_preserves_key_derivation_for_arbitrary_stream_ids() {
        let raw = "orders/created";
        let routing_id = StreamRoutingId::from(raw);

        assert_eq!(routing_id.as_str(), raw);
        assert_eq!(
            ScheduleKey::for_stream(&routing_id),
            ScheduleKey::derive(&schedule_id(raw))
        );
    }

    #[test]
    fn stream_routing_id_accepts_owned_strings() {
        let routing_id = StreamRoutingId::from("orders/created".to_string());
        assert_eq!(routing_id.as_str(), "orders/created");
    }
}
