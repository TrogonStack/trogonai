use super::drive_claim::DriveClaim;

/// One page of the claim scan: the claims found among the drives examined
/// and where the next page resumes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaimPage {
    pub claims: Vec<DriveClaim>,
    /// Opaque position after the last drive examined, to pass back as
    /// `after`. `None` means the scan reached the end; the next scan starts
    /// over.
    pub next: Option<String>,
}
