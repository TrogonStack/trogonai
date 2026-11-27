use crate::file_outcome::FileOutcome;

/// What one file or tree transfer moved.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TransferReport {
    /// Regular files that now match the source.
    pub files: u64,
    /// Directories created or confirmed.
    pub directories: u64,
    /// Bytes sent over the bus (resumed bytes are not counted).
    pub bytes_transferred: u64,
    /// Files continued from a partial copy instead of restarted.
    pub resumed_files: u64,
    /// Files already complete and identical, so nothing moved.
    pub skipped_files: u64,
    /// Symlinks and special files, which a transfer does not copy.
    pub skipped_special: u64,
}

impl TransferReport {
    pub(crate) fn absorb(&mut self, file: FileOutcome) {
        self.files += 1;
        self.bytes_transferred += file.bytes_transferred;
        if file.resumed {
            self.resumed_files += 1;
        }
        if file.skipped {
            self.skipped_files += 1;
        }
    }
}
