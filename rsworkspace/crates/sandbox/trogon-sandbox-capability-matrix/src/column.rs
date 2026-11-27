use trogon_sandbox_contract::ProviderCapabilityProfile;

/// One column of the matrix: a label and the profile the worker declares.
pub struct Column {
    pub label: &'static str,
    pub profile: ProviderCapabilityProfile,
    /// Rows rendered as platform dependent for this worker, each covering every row nested under it.
    pub platform_rows: &'static [&'static str],
}

impl Column {
    pub fn new(label: &'static str, profile: ProviderCapabilityProfile) -> Self {
        Self {
            label,
            profile,
            platform_rows: &[],
        }
    }

    /// Whether `path` is one of [`Column::platform_rows`] or nested under one.
    pub fn is_platform_dependent(&self, path: &str) -> bool {
        self.platform_rows.iter().any(|row| {
            path.strip_prefix(row)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        })
    }
}
