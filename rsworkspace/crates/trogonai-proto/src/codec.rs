use trogon_decider_runtime::{EventData, InvalidSnapshotTypeName, SnapshotTypeName};

pub(crate) fn decode_event_case<Payload, Case>(event: &EventData<'_>) -> Option<Result<Case, buffa::DecodeError>>
where
    Payload: buffa::Message + buffa::MessageName,
    Case: From<Payload>,
{
    (event.event_type == Payload::FULL_NAME).then(|| Payload::decode_from_slice(event.payload).map(Case::from))
}

pub(crate) const fn event_type<Payload>() -> &'static str
where
    Payload: buffa::MessageName,
{
    Payload::FULL_NAME
}

pub(crate) fn snapshot_type<Payload>() -> Result<SnapshotTypeName, InvalidSnapshotTypeName>
where
    Payload: buffa::MessageName,
{
    SnapshotTypeName::new(Payload::FULL_NAME)
}
