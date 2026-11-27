use anyhow::Context;
use windows::Win32::Security::Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile};
use windows::Win32::Security::{FreeSid, PSID};
use windows::core::PCWSTR;

use super::constants::{DESCRIPTION, DISPLAY_NAME};
use crate::isolation::profile_journal::{ProfileJournal, ProfileName};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A profile created for one lease, with no capabilities, so the process
/// inside has no network, no access to the user's files and no access to
/// other processes. The profile is deleted when this is dropped, and is
/// recorded in the journal for as long as it exists.
pub struct Container {
    name: Vec<u16>,
    journal_entry: ProfileName,
    journal: ProfileJournal,
    sid: PSID,
}

// The SID is immutable memory this value owns until it is dropped.
unsafe impl Send for Container {}
unsafe impl Sync for Container {}

impl Container {
    pub fn create(journal: &ProfileJournal) -> anyhow::Result<Self> {
        let journal_entry = ProfileName::generate();
        journal
            .record(&journal_entry)
            .context("recording the AppContainer profile")?;
        let name = wide(journal_entry.as_str());
        let display = wide(DISPLAY_NAME);
        let description = wide(DESCRIPTION);
        let sid = unsafe {
            CreateAppContainerProfile(
                PCWSTR(name.as_ptr()),
                PCWSTR(display.as_ptr()),
                PCWSTR(description.as_ptr()),
                None,
            )
        }
        .inspect_err(|_| journal.forget(&journal_entry))
        .context("CreateAppContainerProfile")?;
        Ok(Self {
            name,
            journal_entry,
            journal: journal.clone(),
            sid,
        })
    }

    pub fn sid(&self) -> PSID {
        self.sid
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteAppContainerProfile(PCWSTR(self.name.as_ptr()));
            FreeSid(self.sid);
        }
        self.journal.forget(&self.journal_entry);
    }
}

/// Deletes the profiles a crashed worker left in the journal, and nothing else.
pub fn sweep(journal: &ProfileJournal) -> usize {
    let mut removed = 0;
    for name in journal.leftover() {
        let wide_name = wide(name.as_str());
        let deleted = unsafe { DeleteAppContainerProfile(PCWSTR(wide_name.as_ptr())) };
        if deleted.is_ok() {
            removed += 1;
        }
        journal.forget(&name);
    }
    removed
}
