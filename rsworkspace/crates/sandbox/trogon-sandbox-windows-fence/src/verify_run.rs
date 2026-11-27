/// The output of one `fence-helper wfp verify` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyRun {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}
