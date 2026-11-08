use crate::a2a_prefix::A2aPrefix;

/// `{prefix}.v1.push.dlq.>`: every dead letter and every mirror of one, for every
/// caller.
///
/// The filter of the gateway's mirror consumer, which tells the two lanes apart by
/// parsing each record with [`crate::nats::subjects::push::PushDlqSubject`] and
/// [`crate::nats::subjects::push::PushDlqMirrorSubject`].
#[derive(Debug)]
pub struct PushDlqAllSubject {
    prefix: A2aPrefix,
}

impl PushDlqAllSubject {
    pub fn new(prefix: &A2aPrefix) -> Self {
        Self { prefix: prefix.clone() }
    }
}

impl std::fmt::Display for PushDlqAllSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.push.dlq.>", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for PushDlqAllSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for PushDlqAllSubject {}

impl super::super::stream::StreamAssignment for PushDlqAllSubject {
    const STREAM: Option<super::super::stream::A2aStream> = None;
}

#[cfg(test)]
mod tests;
