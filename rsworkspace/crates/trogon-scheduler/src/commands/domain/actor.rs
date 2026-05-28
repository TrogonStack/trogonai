use trogonai_proto::actor::v1alpha1 as actor_v1;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScheduleActor(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleActorError {
    Empty,
}

impl std::fmt::Display for ScheduleActorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("actor must not be empty"),
        }
    }
}

impl std::error::Error for ScheduleActorError {}

impl ScheduleActor {
    pub fn parse(value: impl Into<String>) -> Result<Self, ScheduleActorError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ScheduleActorError::Empty);
        }
        if trimmed.len() != value.len() {
            return Err(ScheduleActorError::Empty);
        }
        Ok(Self(value))
    }
}

impl From<&ScheduleActor> for buffa::MessageField<actor_v1::ActorId> {
    fn from(actor: &ScheduleActor) -> Self {
        Self::some(actor_v1::ActorId { value: actor.0.clone() })
    }
}

impl From<ScheduleActor> for buffa::MessageField<actor_v1::ActorId> {
    fn from(actor: ScheduleActor) -> Self {
        Self::some(actor_v1::ActorId { value: actor.0 })
    }
}
