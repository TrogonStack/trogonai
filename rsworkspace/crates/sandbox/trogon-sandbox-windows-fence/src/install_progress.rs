/// Where the install reports what it is about to do and how it goes.
pub trait InstallProgress {
    /// Called once, right before the profile read grant is applied (not when
    /// it is skipped), with one line to show the user.
    fn profile_grant_start(&mut self, _notice: &str) {}
    /// Called with each progress line of the profile read grant.
    fn profile_grant_line(&mut self, _line: &str) {}
}
