use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::Instant;

use ext4_view::Ext4Read;

/// Reads the filesystem image under a total byte budget and a deadline.
pub(super) struct LimitedReader {
    pub(super) file: File,
    pub(super) length: u64,
    pub(super) consumed: u64,
    pub(super) budget: u64,
    pub(super) expires: Instant,
}

impl Ext4Read for LimitedReader {
    fn read(&mut self, start: u64, dst: &mut [u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        if Instant::now() >= self.expires {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "filesystem read deadline exceeded").into());
        }
        let end = start
            .checked_add(dst.len() as u64)
            .ok_or_else(|| std::io::Error::other("filesystem read offset overflow"))?;
        self.consumed = self
            .consumed
            .checked_add(dst.len() as u64)
            .ok_or_else(|| std::io::Error::other("filesystem read budget overflow"))?;
        if end > self.length || self.consumed > self.budget {
            return Err(std::io::Error::other("filesystem read byte budget exceeded").into());
        }
        self.file.seek(SeekFrom::Start(start))?;
        self.file.read_exact(dst)?;
        Ok(())
    }
}
