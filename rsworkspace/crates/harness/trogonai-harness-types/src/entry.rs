//! What a filesystem entry is, independent of the name used to reach it.

use serde::{Deserialize, Serialize};

/// What kind of entry the host reported.
///
/// Only the distinctions the mutation contract acts on: an anchor and every
/// traversed directory must be a directory, and a symlink is never silently
/// accepted as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    SymLink,
    /// Anything else the host reports.
    Other,
}

impl FileKind {
    /// The name this kind is written as in a permission identity.
    ///
    /// Nothing the runtime identifies is one of those: an anchor and every directory above a target must
    /// be a directory, and a target is a file or absent.
    #[must_use]
    pub const fn tag_name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::SymLink => "sym_link",
            Self::Other => "other",
        }
    }
}

/// Which file the host meant, independent of the name used to reach it.
///
/// A name can be re-pointed between the check and the write; the entry it
/// pointed at cannot. Comparing identity is what makes the authorization
/// survive a directory being swapped underneath it.
///
/// A device and inode pair alone does not name one entry over time: a
/// filesystem may hand a freed inode number to the next file it creates, and
/// ext4 does so at once. [`FileIdentity::birth`] is what tells the file that
/// was judged from a later one given the same number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    /// The device the entry lives on.
    pub device: u64,
    /// The entry's inode.
    pub inode: u64,
    /// What kind of entry it is.
    pub kind: FileKind,
    /// When the entry was created, where the host reports it.
    ///
    /// Absent from the serialized form when unreported, so an identity
    /// written before this field existed reads back unchanged.
    #[serde(default, skip_serializing_if = "FileBirth::is_unreported")]
    pub birth: FileBirth,
}

impl FileIdentity {
    /// Whether the host reported a directory.
    #[must_use]
    pub fn is_directory(&self) -> bool {
        self.kind == FileKind::Directory
    }
}

/// When an entry was created, as the host's filesystem recorded it.
///
/// A creation time is fixed for the life of an entry and set afresh for the
/// next one, so it separates two files that were given the same inode number
/// one after the other. It is not a modification time: writing to a file in
/// place leaves it as it was, and the file keeps its identity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileBirth {
    /// The host or its filesystem does not record a creation time.
    ///
    /// Two identities that both carry this are told apart by device and
    /// inode alone, which is all such a host can offer.
    #[default]
    Unreported,
    /// The creation time, relative to the Unix epoch.
    At {
        /// Whole seconds; negative before the epoch.
        seconds: i64,
        /// Nanoseconds past `seconds`, below one billion.
        nanoseconds: u32,
    },
}

impl FileBirth {
    /// Whether the host gave no creation time.
    #[must_use]
    pub const fn is_unreported(&self) -> bool {
        matches!(self, Self::Unreported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_written_before_birth_existed_reads_back_unreported() {
        let legacy = r#"{"device":7,"inode":42,"kind":"file"}"#;
        let identity: FileIdentity = serde_json::from_str(legacy).expect("legacy identity");

        assert_eq!(identity.birth, FileBirth::Unreported);
        assert_eq!(
            serde_json::to_string(&identity).expect("serializes"),
            legacy,
            "an unreported birth adds nothing to the written form"
        );
    }

    #[test]
    fn a_reported_birth_is_written_and_read_back() {
        let identity = FileIdentity {
            device: 7,
            inode: 42,
            kind: FileKind::File,
            birth: FileBirth::At {
                seconds: 1_795_400_000,
                nanoseconds: 414_846_304,
            },
        };
        let written = serde_json::to_string(&identity).expect("serializes");

        assert_eq!(
            serde_json::from_str::<FileIdentity>(&written).expect("reads back"),
            identity
        );
    }
}
