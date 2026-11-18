use crate::PeerMcpErrorCode;

#[derive(serde::Serialize)]
pub(crate) struct PeerMcpErrorContent {
    pub(crate) code: PeerMcpErrorCode,
}
