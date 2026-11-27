use super::command_record::CommandRecord;

/// A custody record together with the bucket revision it was read at.
pub(crate) struct StoredCommand {
    pub(crate) record: CommandRecord,
    pub(crate) revision: u64,
}
