use crate::EventId;

pub trait EventIdentity {
    fn event_id(&self) -> Option<EventId> {
        None
    }
}
