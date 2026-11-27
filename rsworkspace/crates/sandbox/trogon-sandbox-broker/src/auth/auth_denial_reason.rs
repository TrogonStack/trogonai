pub enum AuthDenialReason {
    MissingHeader,
    UnknownKey,
    TenantMismatch,
    PermissionDenied,
    Backend(String),
}

impl std::fmt::Debug for AuthDenialReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHeader => write!(f, "MissingHeader"),
            Self::UnknownKey => write!(f, "UnknownKey"),
            Self::TenantMismatch => write!(f, "TenantMismatch"),
            Self::PermissionDenied => write!(f, "PermissionDenied"),
            Self::Backend(msg) => write!(f, "Backend({msg})"),
        }
    }
}
