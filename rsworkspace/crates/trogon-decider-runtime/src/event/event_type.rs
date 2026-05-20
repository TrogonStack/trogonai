pub trait EventType {
    type Error;

    fn event_type(&self) -> Result<&'static str, Self::Error>;
}
