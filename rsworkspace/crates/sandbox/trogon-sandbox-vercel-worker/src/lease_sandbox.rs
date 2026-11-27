use std::collections::BTreeMap;

/// The Vercel sandbox and session a lease currently runs in.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LeaseSandbox {
    pub name: String,
    pub session_id: String,
    /// The public route Vercel assigned to each port declared at create time.
    pub routes: BTreeMap<u16, String>,
}
