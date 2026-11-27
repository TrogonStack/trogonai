use crate::profile_grant_item::ProfileGrantItem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileGrantResult {
    pub profile: String,
    pub containers: u64,
    pub granted: u64,
    pub files: u64,
    pub denied: u64,
    pub placeholders: u64,
    pub writes: u64,
    pub unchanged: u64,
    pub removed: u64,
    pub skipped: Vec<ProfileGrantItem>,
    pub not_granted: Vec<ProfileGrantItem>,
    pub self_closed: Vec<ProfileGrantItem>,
    pub unverifiable: Vec<ProfileGrantItem>,
    pub unprotected: Vec<ProfileGrantItem>,
    pub stale_allow: Vec<ProfileGrantItem>,
    pub forgotten_roots: Vec<ProfileGrantItem>,
    pub elapsed_ms: u64,
}
