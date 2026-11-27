/// What a synchronous code execution printed and how it ended.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ExecuteResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
