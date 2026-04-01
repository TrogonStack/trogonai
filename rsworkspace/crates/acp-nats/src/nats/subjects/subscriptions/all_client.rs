/// Wildcard: all session client subjects.
#[derive(Debug)]
pub struct AllClientSubject {
    prefix: crate::acp_prefix::AcpPrefix,
}

impl AllClientSubject {
    pub fn new(prefix: &crate::acp_prefix::AcpPrefix) -> Self {
        Self {
            prefix: prefix.clone(),
        }
    }
}

impl std::fmt::Display for AllClientSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.session.*.client.>", self.prefix.as_str())
    }
}

impl async_nats::subject::ToSubject for AllClientSubject {
    fn to_subject(&self) -> async_nats::subject::Subject {
        async_nats::subject::Subject::from(self.to_string().as_str())
    }
}

impl super::super::markers::Subscribable for AllClientSubject {}
