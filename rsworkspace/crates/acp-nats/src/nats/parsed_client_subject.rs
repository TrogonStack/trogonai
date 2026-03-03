use crate::session_id::AcpSessionId;

use super::client_method::ClientMethod;

#[derive(Debug)]
pub struct ParsedClientSubject {
    pub session_id: AcpSessionId,
    pub method: ClientMethod,
}
