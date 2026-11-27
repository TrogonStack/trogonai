use serde::{Deserialize, Serialize};

use crate::{SshSessionId, SshStreamControl};

/// One frame on a session's stream subjects.
///
/// Plain frames carry channel bytes in `data`; `eof` marks the end of that direction. A frame with `ctl` set is a
/// control message for the channel and its `data` is empty.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshStreamFrame {
    pub session_id: SshSessionId,
    pub channel_id: u32,
    pub data: Vec<u8>,
    #[serde(default)]
    pub eof: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctl: Option<SshStreamControl>,
}

impl SshStreamFrame {
    pub fn data(session_id: &SshSessionId, channel_id: u32, data: Vec<u8>) -> Self {
        Self {
            session_id: session_id.clone(),
            channel_id,
            data,
            eof: false,
            ctl: None,
        }
    }

    pub fn eof(session_id: &SshSessionId, channel_id: u32) -> Self {
        Self {
            eof: true,
            ..Self::data(session_id, channel_id, Vec::new())
        }
    }

    pub fn control(session_id: &SshSessionId, channel_id: u32, ctl: SshStreamControl) -> Self {
        Self {
            ctl: Some(ctl),
            ..Self::data(session_id, channel_id, Vec::new())
        }
    }
}
