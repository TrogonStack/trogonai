use super::InvalidationNotice;

/// A notice the scripted stream delivers: its subject, and where it fell in
/// what the consumer delivered, or nothing when that cannot be read.
#[derive(Debug)]
pub(crate) struct ScriptedNotice {
    subject: String,
    sequence: Option<u64>,
}

impl ScriptedNotice {
    pub(crate) fn new(subject: &str, sequence: Option<u64>) -> Self {
        Self {
            subject: subject.to_owned(),
            sequence,
        }
    }
}

impl InvalidationNotice for ScriptedNotice {
    fn subject(&self) -> &str {
        &self.subject
    }

    fn consumer_sequence(&self) -> Result<u64, async_nats::Error> {
        self.sequence.ok_or_else(|| "fake-no-delivery-info".into())
    }
}
