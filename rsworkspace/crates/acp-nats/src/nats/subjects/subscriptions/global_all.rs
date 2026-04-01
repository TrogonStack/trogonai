/// Wildcard: all global agent subjects.
#[derive(Debug)]
pub struct GlobalAllSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl GlobalAllSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for GlobalAllSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.>", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for GlobalAllSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for GlobalAllSubject {}
