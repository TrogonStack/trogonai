use std::collections::BTreeMap;

/// One PTY process to start.
#[derive(Debug, Clone)]
pub struct PtyRequest {
    pub tag: String,
    pub cmd: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub user: String,
    pub cols: u16,
    pub rows: u16,
}
