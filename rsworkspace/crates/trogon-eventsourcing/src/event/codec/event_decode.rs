#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct EventData<'a> {
    pub event_type: &'a str,
    pub payload: &'a [u8],
}

impl<'a> EventData<'a> {
    pub const fn new(event_type: &'a str, payload: &'a [u8]) -> Self {
        Self { event_type, payload }
    }
}

pub trait EventDecode: Sized {
    type Error;

    fn decode(event: EventData<'_>) -> Result<Self, Self::Error>;
}
