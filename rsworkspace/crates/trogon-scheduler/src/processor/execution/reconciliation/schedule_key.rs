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
mod tests;
