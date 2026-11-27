use trogon_sandbox_contract::OutputStream;

/// Lines a guest printed, tagged with the stream they were written to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapturedOutput {
    pub lines: Vec<(OutputStream, String)>,
}

impl CapturedOutput {
    /// All captured text concatenated in print order, ignoring the stream.
    pub fn text(&self) -> String {
        self.lines.iter().map(|(_, text)| text.as_str()).collect()
    }
}
