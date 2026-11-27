use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, ensure};
use std::os::unix::fs::MetadataExt;

mod constants;
use constants::FREE_MARGIN;

#[derive(Default)]
struct Reservations(Mutex<HashMap<u64, u64>>);

pub struct DiskReservation {
    ledger: Arc<Reservations>,
    filesystem: u64,
    bytes: u64,
}

impl Reservations {
    fn reserve(
        self: &Arc<Self>,
        filesystem: u64,
        bytes: u64,
        available: impl FnOnce() -> anyhow::Result<u64>,
    ) -> anyhow::Result<Arc<DiskReservation>> {
        let mut reserved = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("disk reservation ledger is poisoned"))?;
        let used = reserved.get(&filesystem).copied().unwrap_or(0);
        let total = used.checked_add(bytes).context("disk reservations overflow u64")?;
        ensure!(
            available()?
                >= total
                    .checked_add(FREE_MARGIN)
                    .context("disk reservation margin overflows")?,
            "insufficient free disk space for concurrent lease reservations"
        );
        reserved.insert(filesystem, total);
        Ok(Arc::new(DiskReservation {
            ledger: Arc::clone(self),
            filesystem,
            bytes,
        }))
    }
}

pub fn reserve(path: &Path, bytes: u64) -> anyhow::Result<Arc<DiskReservation>> {
    static LEDGER: OnceLock<Arc<Reservations>> = OnceLock::new();
    let filesystem = std::fs::metadata(path)?.dev();
    LEDGER
        .get_or_init(|| Arc::new(Reservations::default()))
        .reserve(filesystem, bytes, || Ok(fs2::available_space(path)?))
}

impl Drop for DiskReservation {
    fn drop(&mut self) {
        let mut reserved = self.ledger.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(bytes) = reserved.get_mut(&self.filesystem) {
            *bytes -= self.bytes;
            if *bytes == 0 {
                reserved.remove(&self.filesystem);
            }
        }
    }
}

#[cfg(test)]
mod tests;
