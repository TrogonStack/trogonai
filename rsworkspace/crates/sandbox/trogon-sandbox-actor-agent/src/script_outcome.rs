/// How a provisioning script ended, once it was started at all.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum ScriptOutcome {
    Succeeded,
    Failed { code: i32, stderr: String },
}
