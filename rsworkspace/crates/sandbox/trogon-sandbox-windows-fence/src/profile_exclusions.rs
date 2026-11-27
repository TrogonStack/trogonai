/// The paths a profile read grant keeps unreadable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileExclusions {
    pub exclusions: Vec<String>,
    /// Directories that are not granted as a whole; paths inside them may be.
    pub ungranted_only: Vec<String>,
}
