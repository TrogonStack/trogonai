/// Numbering of the control frames sent to one bridge. The first failure ends control for good, since whether the
/// bridge applied the frame is unknown.
#[derive(Debug, Default)]
pub(crate) struct OrdinaryControl {
    pub(crate) sequence: u64,
    pub(crate) broken: bool,
}
