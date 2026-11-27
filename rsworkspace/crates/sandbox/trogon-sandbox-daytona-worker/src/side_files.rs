use crate::constants::COMMAND_FILE_PREFIX;

/// Where the sandbox keeps what one command leaves behind: its streams, its exit status, its deadline receipt and
/// the pipe that carries its stdin.
#[derive(Debug, Clone)]
pub(crate) struct SideFiles {
    pub(crate) out: String,
    pub(crate) err: String,
    pub(crate) exit: String,
    pub(crate) status: String,
    pub(crate) stdin: String,
}

impl SideFiles {
    pub(crate) fn new(execution_id: &str) -> Self {
        let prefix = format!("{COMMAND_FILE_PREFIX}{execution_id}");
        Self {
            out: format!("{prefix}.out"),
            err: format!("{prefix}.err"),
            exit: format!("{prefix}.exit"),
            status: format!("{prefix}.status"),
            stdin: format!("{prefix}.stdin"),
        }
    }

    /// Every file a command may leave, for the cleanup after its result is read.
    pub(crate) fn all(&self) -> [&str; 5] {
        [&self.exit, &self.out, &self.err, &self.status, &self.stdin]
    }
}
