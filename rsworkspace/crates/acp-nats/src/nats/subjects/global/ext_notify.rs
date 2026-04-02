/// Core NATS publish (fire-and-forget notification).
#[derive(Debug)]
pub struct ExtNotifySubject {
    prefix: crate::acp_prefix::AcpPrefix,
    method: crate::ext_method_name::ExtMethodName,
}

impl ExtNotifySubject {
    pub fn new(
        prefix: &crate::acp_prefix::AcpPrefix,
        method: &crate::ext_method_name::ExtMethodName,
    ) -> Self {
        Self {
            prefix: prefix.clone(),
            method: method.clone(),
        }
    }
}

impl std::fmt::Display for ExtNotifySubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.agent.ext.{}", self.prefix.as_str(), self.method)
    }
}

impl super::super::markers::Publishable for ExtNotifySubject {}

impl super::super::stream::StreamAssignment for ExtNotifySubject {
    const STREAM: Option<super::super::stream::AcpStream> =
        Some(super::super::stream::AcpStream::GlobalExt);
}
