//! The folder walk on a host whose links, write checks and failures are maps, so the walk runs the same on any
//! machine. Every folder `stat` reaches reports the owner of one real folder that nobody but its owner can write.
#![cfg(unix)]

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::{OsStr, OsString},
    fs,
    io::{self, ErrorKind},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::SystemTime,
};

use super::{
    ByIds, Identity, Owner, owners_or_folders_allow, registry_tool_writable, settled_by_ids, walk_finds_writable_folder,
};
use crate::{
    adapters::{
        CanonicalizePath, CheckReadable, CheckWritable, CurrentDirectory, DirectoryEntry, DirectoryEntryKind,
        EffectiveGroupId, EffectiveUserId, EnvironmentVariable, ExecuteFile, ExecuteFileOutcome, ExecuteFileRequest,
        HomeDirectory, HostIdentity, LinkMetadata, NativeHost, OpenDirectoryNoFollow, OperatingSystem, ReadDeviceId,
        ReadDirectory, ReadFile, ReadFileHead, ReadLink, ReadMetadata, ReadSymlinkMetadata, RealGroupId, RealUserId,
        SupplementaryGroupIds, WallClock,
    },
    settings_resolve_platform_kind::PlatformKind,
};

const OTHER_USER: u32 = 502;
const WHEEL: u32 = 0;

fn owner(uid: u32, gid: u32, mode: u32) -> Option<Owner> {
    Some(Owner { uid, gid, mode })
}

/// A user id no machine running the tests owns a folder as.
const WALKER: u32 = 4_000_000;
const DOCUMENT: &str = "/etc/claude-code/managed-settings.json";

#[derive(Clone, Default)]
struct WalkHost {
    /// The real folder whose owner every `stat` reports.
    stat_from: PathBuf,
    /// Paths `stat` cannot read.
    unstated: BTreeSet<PathBuf>,
    /// Paths `lstat` cannot read.
    unlinked: BTreeSet<PathBuf>,
    /// Symbolic links and their text.
    links: BTreeMap<PathBuf, PathBuf>,
    /// Symbolic links whose text cannot be read.
    unreadable_links: BTreeSet<PathBuf>,
    /// Write checks that do not fail with `EACCES`: `None` succeeds, `Some` fails with that kind.
    writes: BTreeMap<PathBuf, Option<ErrorKind>>,
    euid: Option<u32>,
    uid: Option<u32>,
}

impl WalkHost {
    fn new(stat_from: &Path) -> Self {
        Self {
            stat_from: stat_from.to_path_buf(),
            euid: Some(WALKER),
            uid: Some(WALKER),
            ..Self::default()
        }
    }

    fn write(mut self, path: &str, answer: Option<ErrorKind>) -> Self {
        self.writes.insert(PathBuf::from(path), answer);
        self
    }

    fn link(mut self, path: &str, target: &str) -> Self {
        self.links.insert(PathBuf::from(path), PathBuf::from(target));
        self
    }
}

fn missing() -> io::Error {
    io::Error::from(ErrorKind::NotFound)
}

impl ReadFile for WalkHost {
    type Error = io::Error;

    fn read_file(&self, _path: &Path) -> Result<Vec<u8>, io::Error> {
        Err(missing())
    }
}

impl CheckReadable for WalkHost {
    type Error = io::Error;

    fn check_readable(&self, _path: &Path) -> Result<(), io::Error> {
        Err(missing())
    }
}

impl ReadDirectory for WalkHost {
    type Error = io::Error;

    fn read_directory(&self, _path: &Path) -> Result<Vec<DirectoryEntry>, io::Error> {
        Err(missing())
    }
}

impl ReadSymlinkMetadata for WalkHost {
    type Error = io::Error;

    fn symlink_metadata(&self, path: &Path) -> Result<LinkMetadata, io::Error> {
        if self.unlinked.contains(path) {
            return Err(io::Error::from(ErrorKind::PermissionDenied));
        }
        let kind = if self.links.contains_key(path) || self.unreadable_links.contains(path) {
            DirectoryEntryKind::Symlink
        } else {
            DirectoryEntryKind::Directory
        };
        Ok(LinkMetadata { kind, len: 0 })
    }
}

impl ReadLink for WalkHost {
    type Error = io::Error;

    fn read_link(&self, path: &Path) -> Result<PathBuf, io::Error> {
        self.links
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::from(ErrorKind::PermissionDenied))
    }
}

impl ReadMetadata for WalkHost {
    type Error = io::Error;

    fn metadata(&self, path: &Path) -> Result<fs::Metadata, io::Error> {
        if self.unstated.contains(path) {
            return Err(io::Error::from(ErrorKind::PermissionDenied));
        }
        fs::metadata(&self.stat_from)
    }
}

impl CheckWritable for WalkHost {
    type Error = io::Error;

    fn check_writable(&self, path: &Path) -> Result<(), io::Error> {
        match self.writes.get(path) {
            Some(None) => Ok(()),
            Some(Some(kind)) => Err(io::Error::from(*kind)),
            None => Err(io::Error::from(ErrorKind::PermissionDenied)),
        }
    }
}

impl CanonicalizePath for WalkHost {
    type Error = io::Error;

    fn canonicalize(&self, _path: &Path) -> Result<PathBuf, io::Error> {
        Err(io::Error::from(ErrorKind::Unsupported))
    }
}

impl EffectiveUserId for WalkHost {
    fn effective_user_id(&self) -> Option<u32> {
        self.euid
    }
}

impl RealUserId for WalkHost {
    fn real_user_id(&self) -> Option<u32> {
        self.uid
    }
}

impl EffectiveGroupId for WalkHost {
    fn effective_group_id(&self) -> Option<u32> {
        None
    }
}

impl RealGroupId for WalkHost {
    fn real_group_id(&self) -> Option<u32> {
        None
    }
}

impl SupplementaryGroupIds for WalkHost {
    type Error = io::Error;

    fn supplementary_group_ids(&self) -> Option<Result<Vec<u32>, io::Error>> {
        None
    }
}

impl OpenDirectoryNoFollow for WalkHost {
    type Error = io::Error;

    fn open_directory_no_follow(&self, _path: &Path) -> Result<(), io::Error> {
        Err(io::Error::from(ErrorKind::Unsupported))
    }
}

impl ReadFileHead for WalkHost {
    type Error = io::Error;

    fn read_file_head(&self, _path: &Path, _limit: usize) -> Result<Vec<u8>, io::Error> {
        Err(missing())
    }
}

impl ReadDeviceId for WalkHost {
    type Error = io::Error;

    fn device_id(&self, _path: &Path) -> Result<u64, io::Error> {
        Err(missing())
    }
}

impl HomeDirectory for WalkHost {
    type Error = io::Error;

    fn home_directory(&self) -> Result<PathBuf, io::Error> {
        Err(missing())
    }
}

impl ExecuteFile for WalkHost {
    fn execute_file(&self, request: &ExecuteFileRequest<'_>) -> ExecuteFileOutcome {
        NativeHost.execute_file(request)
    }
}

impl HostIdentity for WalkHost {
    fn operating_system(&self) -> OperatingSystem {
        OperatingSystem::Linux
    }

    fn username(&self) -> Option<String> {
        None
    }
}

impl EnvironmentVariable for WalkHost {
    fn var_os(&self, _key: &OsStr) -> Option<OsString> {
        None
    }
}

impl CurrentDirectory for WalkHost {
    type Error = io::Error;

    fn current_dir(&self) -> Result<PathBuf, io::Error> {
        Ok(PathBuf::from("/"))
    }
}

impl WallClock for WalkHost {
    type Time = SystemTime;

    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
}

/// A real folder only its owner can write, whose owner is never [`WALKER`].
fn sealed_folder() -> io::Result<tempfile::TempDir> {
    let folder = tempfile::tempdir()?;
    fs::set_permissions(folder.path(), fs::Permissions::from_mode(0o755))?;
    Ok(folder)
}

fn walker() -> Identity {
    Identity {
        uids: vec![WALKER],
        groups: Vec::new(),
    }
}

/// `GP` upstream once the owners of the document and its folder both say no.
fn sealed_owners_allow(host: &WalkHost, path: &str, certain: &mut HashMap<PathBuf, bool>) -> bool {
    owners_or_folders_allow(
        host,
        Path::new(path),
        &walker(),
        owner(OTHER_USER, WHEEL, 0o644),
        owner(OTHER_USER, WHEEL, 0o755),
        false,
        certain,
    )
}

#[test]
fn a_read_only_document_under_sealed_folders_cannot_be_rewritten() -> io::Result<()> {
    let folder = sealed_folder()?;
    let host = WalkHost::new(folder.path()).write(DOCUMENT, Some(ErrorKind::ReadOnlyFilesystem));
    let mut certain = HashMap::new();
    assert!(!sealed_owners_allow(&host, DOCUMENT, &mut certain));
    assert_eq!(certain.get(Path::new("/")), Some(&true));
    assert_eq!(certain.get(Path::new("/etc")), Some(&true));
    assert_eq!(certain.get(Path::new("/etc/claude-code")), Some(&true));
    // The folders judged once are not asked about again.
    let forgetful = WalkHost {
        unstated: BTreeSet::from([PathBuf::from("/"), PathBuf::from("/etc")]),
        ..host
    };
    assert!(!sealed_owners_allow(&forgetful, DOCUMENT, &mut certain));
    Ok(())
}

#[test]
fn a_write_check_that_fails_for_another_reason_counts_as_writable() -> io::Result<()> {
    let folder = sealed_folder()?;
    let host = WalkHost::new(folder.path()).write(DOCUMENT, Some(ErrorKind::NotFound));
    assert!(sealed_owners_allow(&host, DOCUMENT, &mut HashMap::new()));
    let host = WalkHost::new(folder.path()).write(DOCUMENT, None);
    assert!(sealed_owners_allow(&host, DOCUMENT, &mut HashMap::new()));
    Ok(())
}

#[test]
fn an_unverified_link_counts_as_writable() -> io::Result<()> {
    let folder = sealed_folder()?;
    let host = WalkHost::new(folder.path());
    let allowed = owners_or_folders_allow(
        &host,
        Path::new(DOCUMENT),
        &walker(),
        owner(OTHER_USER, WHEEL, 0o644),
        owner(OTHER_USER, WHEEL, 0o755),
        true,
        &mut HashMap::new(),
    );
    assert!(allowed);
    Ok(())
}

#[test]
fn a_folder_on_the_way_the_user_can_write_lets_the_document_be_replaced() -> io::Result<()> {
    let folder = sealed_folder()?;
    let host = WalkHost::new(folder.path()).write("/etc", None);
    let mut certain = HashMap::new();
    assert!(sealed_owners_allow(&host, DOCUMENT, &mut certain));
    assert_eq!(certain.get(Path::new("/etc")), Some(&false));
    assert_eq!(certain.get(Path::new("/etc/claude-code")), None);
    Ok(())
}

#[test]
fn a_folder_that_cannot_be_examined_counts_as_writable() -> io::Result<()> {
    let folder = sealed_folder()?;
    let unstated = WalkHost {
        unstated: BTreeSet::from([PathBuf::from("/etc/claude-code")]),
        ..WalkHost::new(folder.path())
    };
    assert!(walk_finds_writable_folder(
        &unstated,
        Path::new(DOCUMENT),
        &walker(),
        &mut HashMap::new()
    ));
    let unlinked = WalkHost {
        unlinked: BTreeSet::from([PathBuf::from("/etc/claude-code")]),
        ..WalkHost::new(folder.path())
    };
    assert!(walk_finds_writable_folder(
        &unlinked,
        Path::new(DOCUMENT),
        &walker(),
        &mut HashMap::new()
    ));
    let unreadable = WalkHost {
        unreadable_links: BTreeSet::from([PathBuf::from("/etc/claude-code")]),
        ..WalkHost::new(folder.path())
    };
    assert!(walk_finds_writable_folder(
        &unreadable,
        Path::new(DOCUMENT),
        &walker(),
        &mut HashMap::new()
    ));
    Ok(())
}

#[test]
fn a_link_that_climbs_out_of_where_it_points_counts_as_writable() -> io::Result<()> {
    let folder = sealed_folder()?;
    let host = WalkHost::new(folder.path())
        .link("/etc/claude-code", "managed/../../elsewhere")
        .link("/etc/climbing", "./../..");
    assert!(walk_finds_writable_folder(
        &host,
        Path::new(DOCUMENT),
        &walker(),
        &mut HashMap::new()
    ));
    assert!(walk_finds_writable_folder(
        &host,
        Path::new("/etc/climbing/managed-settings.json"),
        &walker(),
        &mut HashMap::new()
    ));
    Ok(())
}

#[test]
fn the_walk_starts_over_where_a_link_points() -> io::Result<()> {
    let folder = sealed_folder()?;
    let absolute = WalkHost::new(folder.path()).link("/etc/claude-code", "/opt/managed");
    assert!(!walk_finds_writable_folder(
        &absolute,
        Path::new(DOCUMENT),
        &walker(),
        &mut HashMap::new()
    ));
    let absolute = absolute.write("/opt/managed", None);
    assert!(walk_finds_writable_folder(
        &absolute,
        Path::new(DOCUMENT),
        &walker(),
        &mut HashMap::new()
    ));
    let relative = WalkHost::new(folder.path()).link("/etc/claude-code", "../srv/managed");
    assert!(!walk_finds_writable_folder(
        &relative,
        Path::new(DOCUMENT),
        &walker(),
        &mut HashMap::new()
    ));
    let relative = relative.write("/srv", None);
    assert!(walk_finds_writable_folder(
        &relative,
        Path::new(DOCUMENT),
        &walker(),
        &mut HashMap::new()
    ));
    Ok(())
}

#[test]
fn a_link_loop_counts_as_writable_once_the_walk_gives_up() -> io::Result<()> {
    let folder = sealed_folder()?;
    let host = WalkHost::new(folder.path()).link("/loop", "/loop");
    assert!(walk_finds_writable_folder(
        &host,
        Path::new("/loop/managed-settings.json"),
        &walker(),
        &mut HashMap::new()
    ));
    Ok(())
}

#[test]
fn the_ids_settle_the_answer_before_the_owners_when_they_can() -> io::Result<()> {
    let folder = sealed_folder()?;
    let host = WalkHost::new(folder.path());
    let managed = Path::new("/mnt/c/Program Files/ClaudeCode/managed-settings.json");
    assert!(matches!(
        settled_by_ids(&host, PlatformKind::Wsl, true, managed),
        ByIds::Settled(false)
    ));
    assert!(matches!(
        settled_by_ids(&host, PlatformKind::Wsl, true, Path::new(DOCUMENT)),
        ByIds::AskOwners(WALKER, Some(WALKER))
    ));
    assert!(matches!(
        settled_by_ids(&host, PlatformKind::Windows, false, Path::new(DOCUMENT)),
        ByIds::Settled(false)
    ));
    let root = WalkHost {
        uid: Some(0),
        ..host.clone()
    };
    assert!(matches!(
        settled_by_ids(&root, PlatformKind::Linux, false, Path::new(DOCUMENT)),
        ByIds::Settled(true)
    ));
    let idless = WalkHost {
        euid: None,
        uid: None,
        ..host
    };
    assert!(matches!(
        settled_by_ids(&idless, PlatformKind::Linux, false, Path::new(DOCUMENT)),
        ByIds::Settled(false)
    ));
    Ok(())
}

#[test]
fn the_registry_tool_under_sealed_folders_cannot_be_replaced() -> io::Result<()> {
    let folder = sealed_folder()?;
    let host = WalkHost::new(folder.path());
    assert!(!registry_tool_writable(&host, PlatformKind::Wsl, false));
    let host = host.write("/mnt/c", None);
    assert!(registry_tool_writable(&host, PlatformKind::Wsl, false));
    Ok(())
}
