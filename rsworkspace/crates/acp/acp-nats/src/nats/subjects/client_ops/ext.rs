/// Agent -> bridge. An extension call: core NATS request/reply for a request,
/// a plain publish for a notification. The method name is the subject's last
/// token, exactly as the agent sent it.
#[derive(Debug)]
pub struct ExtSubject {
    prefix: crate::acp_prefix::AcpPrefix,
    session_id: crate::session_id::AcpSessionId,
    method: crate::ext_method_name::ExtMethodName,
}

impl ExtSubject {
    pub fn new(
        prefix: &crate::acp_prefix::AcpPrefix,
        session_id: &crate::session_id::AcpSessionId,
        method: &crate::ext_method_name::ExtMethodName,
    ) -> Self {
        Self {
            prefix: prefix.clone(),
            session_id: session_id.clone(),
            method: method.clone(),
        }
    }
}

impl std::fmt::Display for ExtSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.v1.session.{}.client.ext.{}",
            self.prefix.as_str(),
            self.session_id.as_str(),
            self.method
        )
    }
}

impl super::super::markers::ClientRequestable for ExtSubject {}

impl super::super::markers::ClientPublishable for ExtSubject {}

impl super::super::stream::StreamAssignment for ExtSubject {
    const STREAM: Option<super::super::stream::AcpStream> = Some(super::super::stream::AcpStream::ClientOps);
}
