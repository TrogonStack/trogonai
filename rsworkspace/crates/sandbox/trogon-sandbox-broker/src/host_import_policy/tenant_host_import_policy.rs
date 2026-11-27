pub struct TenantHostImportPolicy {
    pub allow_domains: Vec<String>,
    pub max_body_bytes: u32,
    pub max_timeout_seconds: u32,
}
