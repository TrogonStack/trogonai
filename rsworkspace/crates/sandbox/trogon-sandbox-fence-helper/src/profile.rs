//! The profile read grant - `fence-helper profile grant | forget | status`.
//! Makes the real user's profile readable to the sandbox GROUP, except
//! for the credential stores [`crate::profile_plan`] lists. That module
//! decides what each directory gets; this one reads the tree, writes one
//! ACE per path through the converge of [`crate::persist`], and keeps the
//! record (session database and registry marker).
//! The entries change only while no sandboxed process exists: grant and
//! forget hold the init lock for their whole run and refuse while any
//! holder is live, so a session's view is fixed from its start to its end.
//! Nothing is written through a reparse point; every object is opened
//! with `FILE_FLAG_OPEN_REPARSE_POINT` and written through that handle.
use crate::context::Context;
use crate::error::{FenceHelperError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};

use crate::acl;
use crate::path_id::{self, FileId, Located};
use crate::persist::{self, Ids, OpenObject};
use crate::profile_plan::{self as plan, Entry, Kind, LinkTarget, Name, Node, Rules, Tree};
use crate::reg;
use crate::util::{OwnedHandle, pcwstr, wstr};
use plan::Listing;
use serde_json::json;
use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::NtOpenFile;
use windows::Win32::Foundation::{
    ERROR_NO_MORE_FILES, OBJECT_ATTRIBUTE_FLAGS, RtlNtStatusToDosError, UNICODE_STRING, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_BOTH_DIR_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdBothDirectoryInfo,
    FileIdBothDirectoryRestartInfo, GetFileInformationByHandle, GetFileInformationByHandleEx, OPEN_EXISTING,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::IO::{DeviceIoControl, IO_STATUS_BLOCK};
use windows::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SESSION, WTS_INFO_CLASS, WTSDomainName, WTSFreeMemory, WTSQuerySessionInformationW, WTSUserName,
};
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};

// ─── Where the profile is ───────────────────────────────────────────

/// The profile and its two application-data folders, as canonical
/// (`\\?\`-prefixed) paths.
#[derive(Debug, Clone)]
pub struct Layout {
    pub profile: String,
    pub roaming: Option<String>,
    pub local: Option<String>,
    /// The calling user's own settings of the variables that move a
    /// tool's home or a protected store ([`plan::ENV_STORES`],
    /// `RUSTUP_HOME`), read once.
    pub own_env: Vec<(String, String)>,
}

impl Layout {
    /// The calling user's profile, from the known folders. `test_profile`,
    /// which only a `test-hooks` build can pass, stands in for it.
    pub fn resolve(test_profile: Option<&str>) -> Result<Layout> {
        let canon = |p: &str| -> Result<String> {
            let (c, is_dir) = path_id::canonicalize_path(p)
                .map_err(|e| FenceHelperError::msg(format!("profile directory '{p}': {e}")))?;
            if !is_dir {
                return Err(FenceHelperError::msg(format!("'{p}' is not a directory")));
            }
            Ok(c)
        };
        if let Some(t) = test_profile {
            let profile = canon(t)?;
            let sub = |rel: &str| canon(&format!("{profile}\\{rel}")).ok();
            return Ok(Layout {
                roaming: sub(r"AppData\Roaming"),
                local: sub(r"AppData\Local"),
                profile,
                own_env: Layout::read_own_env(),
            });
        }
        Ok(Layout {
            profile: canon(&known_folder(&FOLDERID_Profile)?)?,
            roaming: known_folder(&FOLDERID_RoamingAppData).and_then(|p| canon(&p)).ok(),
            local: known_folder(&FOLDERID_LocalAppData).and_then(|p| canon(&p)).ok(),
            own_env: Layout::read_own_env(),
        })
    }

    fn read_own_env() -> Vec<(String, String)> {
        plan::ENV_STORES
            .iter()
            .map(|(var, _)| *var)
            .chain(["RUSTUP_HOME"])
            .filter_map(|var| Some((var.to_owned(), std::env::var(var).ok()?)))
            .collect()
    }

    /// The calling user's own value of `var`, if it was set.
    fn own(&self, var: &str) -> Option<String> {
        self.own_env
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(var))
            .map(|(_, v)| v.clone())
    }

    /// `folder` as components beneath the profile; `None` when it is
    /// not inside it (a redirected folder).
    fn rel(&self, folder: &Option<String>) -> Option<Vec<String>> {
        let f = folder.as_deref()?;
        plan::rel_components(&self.profile, f).ok().filter(|c| !c.is_empty())
    }
}

fn known_folder(id: &windows::core::GUID) -> Result<String> {
    let p = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.context("SHGetKnownFolderPath")?;
    let s = unsafe { p.to_string() };
    unsafe { CoTaskMemFree(Some(p.0 as *const std::ffi::c_void)) };
    s.context("known-folder path is not valid Unicode")
}

fn join(profile: &str, rel: &[String]) -> String {
    let mut p = profile.trim_end_matches('\\').to_owned();
    for c in rel {
        p.push('\\');
        p.push_str(c);
    }
    p
}

// ─── Opening without following ──────────────────────────────────────

const FILE_LIST_DIRECTORY: u32 = 0x1;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;
/// To read and write an object's DACL.
const ACCESS_STAMP: u32 = READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES;
/// To list a directory.
const ACCESS_LIST: u32 = FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES;

const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_REPARSE_POINT: u32 = 0x400;
const ATTR_OFFLINE: u32 = 0x1000;
const ATTR_RECALL_ON_OPEN: u32 = 0x4_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

/// Why an object with these attributes is left alone, if it is: a reparse
/// point redirects, and an offline or recall-on-open directory is a cloud
/// placeholder whose listing would fetch it.
pub fn skip_reason(attrs: u32, is_dir: bool) -> Option<&'static str> {
    if attrs & ATTR_REPARSE_POINT != 0 {
        return Some("reparse point (junction, symbolic link or placeholder)");
    }
    let cloud = ATTR_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS;
    if is_dir && attrs & cloud != 0 {
        return Some("cloud placeholder directory");
    }
    None
}

/// An object opened by [`open_no_follow`], with what was read from
/// the handle.
struct Opened {
    h: OwnedHandle,
    attrs: u32,
    id: FileId,
    is_dir: bool,
    /// Opened for its access list only ([`open_bare`]): attributes
    /// and identity are its parent's word, and the handle says nothing
    /// else about the object.
    bare: bool,
}

impl Opened {
    fn object(&self) -> OpenObject {
        OpenObject {
            handle: self.h.raw(),
            id: self.id,
            is_dir: self.is_dir,
        }
    }
}

/// Open `path` itself: if its last component is a link, the link, not
/// what it points at. Attributes and identity come from the handle.
fn open_no_follow(path: &str, access: u32) -> Result<Opened> {
    let w = wstr(path);
    let h = unsafe {
        CreateFileW(
            pcwstr(&w),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    }
    .with_context(|| format!("open '{path}'"))?;
    if h == INVALID_HANDLE_VALUE {
        return Err(FenceHelperError::msg(format!("open '{path}': invalid handle")));
    }
    let h = OwnedHandle(h);
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(h.raw(), &mut info) }.with_context(|| format!("attributes of '{path}'"))?;
    let id = path_id::file_id_from_handle(h.raw()).with_context(|| format!("file id of '{path}'"))?;
    Ok(Opened {
        attrs: info.dwFileAttributes,
        is_dir: info.dwFileAttributes & ATTR_DIRECTORY != 0,
        id,
        h,
        bare: false,
    })
}

/// Open `path` itself with EXACTLY `access` (`NtOpenFile`). `CreateFileW`
/// always adds `SYNCHRONIZE` and `FILE_READ_ATTRIBUTES`, which an object
/// that denies reading refuses even to its owner.
fn open_bare(path: &str, access: u32) -> Result<OwnedHandle> {
    const OBJ_CASE_INSENSITIVE: u32 = 0x40;
    const FILE_SHARE_ALL: u32 = 0x7;
    const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x4000;
    const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let Some(rest) = path.strip_prefix(r"\\?\") else {
        return Err(FenceHelperError::msg(format!("open '{path}': not a full path")));
    };
    let mut wide: Vec<u16> = format!(r"\??\{rest}").encode_utf16().collect();
    let bytes = u16::try_from(wide.len() * 2).map_err(|_| FenceHelperError::msg(format!("open '{path}': too long")))?;
    let name = UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes,
        Buffer: windows::core::PWSTR(wide.as_mut_ptr()),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        ObjectName: &name,
        Attributes: OBJECT_ATTRIBUTE_FLAGS(OBJ_CASE_INSENSITIVE),
        ..Default::default()
    };
    let mut handle = HANDLE::default();
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: every pointer is to a local that outlives the call;
    // `name` points into `wide`, which does too.
    let status = unsafe {
        NtOpenFile(
            &mut handle,
            access,
            &attributes,
            &mut status_block,
            FILE_SHARE_ALL,
            FILE_OPEN_FOR_BACKUP_INTENT | FILE_OPEN_REPARSE_POINT,
        )
    };
    if status.is_err() {
        // As a Win32 error, so it classifies like any other open.
        let code = unsafe { RtlNtStatusToDosError(status) };
        return Err(windows::core::Error::from(WIN32_ERROR(code))).with_context(|| format!("open '{path}'"));
    }
    Ok(OwnedHandle(handle))
}

/// Whether the system refused `e` for lack of access - as opposed to
/// any refusal this code makes itself.
pub(crate) fn is_access_denied(e: &FenceHelperError) -> bool {
    e.system_error()
        .is_some_and(|we| we.code() == windows::Win32::Foundation::ERROR_ACCESS_DENIED.into())
}

/// [`open_no_follow`] with the access a DACL read and write needs. Refused
/// that, it opens for `READ_CONTROL | WRITE_DAC` alone and takes the
/// object's attributes and identity from its parent's listing.
fn open_for_stamp(path: &str, is_dir: Option<bool>) -> Result<Opened> {
    let full = || -> Result<Opened> {
        let (is_dir, probe) = match is_dir {
            Some(d) => (d, None),
            None => {
                let probe = open_no_follow(path, FILE_READ_ATTRIBUTES)?;
                (probe.is_dir, Some(probe.id))
            }
        };
        let o = if is_dir {
            open_no_follow(path, ACCESS_STAMP | FILE_LIST_DIRECTORY).or_else(|_| open_no_follow(path, ACCESS_STAMP))?
        } else {
            open_no_follow(path, ACCESS_STAMP)?
        };
        if probe.is_some_and(|id| id != o.id) {
            return Err(FenceHelperError::msg(format!(
                "'{path}' was replaced while it was being opened"
            )));
        }
        Ok(o)
    };
    match full() {
        Err(e) if is_access_denied(&e) => {
            let h = open_bare(path, READ_CONTROL | WRITE_DAC)?;
            let (attrs, id) = facts_from_parent(path)?;
            Ok(Opened {
                h,
                attrs,
                id,
                is_dir: attrs & ATTR_DIRECTORY != 0,
                bare: true,
            })
        }
        other => other,
    }
}

/// The attributes and identity of `path` as its parent directory
/// lists it: for an object that can be opened for its access list
/// only.
fn facts_from_parent(path: &str) -> Result<(u32, FileId)> {
    let (parent, name) = path
        .trim_end_matches('\\')
        .rsplit_once('\\')
        .ok_or_else(|| FenceHelperError::msg(format!("'{path}' has no parent directory")))?;
    let dir = open_no_follow(parent, ACCESS_LIST)?;
    verify_location(&dir, parent)?;
    let entry = list_dir_raw(dir.h.raw())?
        .into_iter()
        .find(|e| e.long.eq_ignore_ascii_case(name) || e.long.to_lowercase() == name.to_lowercase())
        .ok_or_else(|| FenceHelperError::msg(format!("'{path}' is not listed in its directory")))?;
    // The 64-bit file reference, in the 128-bit form the handle would
    // report on the same volume.
    let mut id128 = [0u8; 16];
    id128[..8].copy_from_slice(&entry.file_id.to_le_bytes());
    Ok((
        entry.attrs,
        FileId {
            volume_serial: dir.id.volume_serial,
            id128,
        },
    ))
}

/// Fail unless the object behind `o` is where `path` says: a
/// directory on the way that is a link would have taken the open
/// somewhere else.
fn verify_location(o: &Opened, path: &str) -> Result<()> {
    // Found by name in a parent that was itself verified.
    if o.bare {
        return Ok(());
    }
    let at = path_id::final_path_of(o.h.raw()).with_context(|| format!("final path of '{path}'"))?;
    if at.to_lowercase() != path.to_lowercase() {
        return Err(FenceHelperError::msg(format!(
            "'{path}' resolves to '{at}': a directory on the way is a link"
        )));
    }
    Ok(())
}

/// The entries of the directory open on `h` (which needs
/// `FILE_LIST_DIRECTORY`), without `.` and `..`.
fn list_dir(h: HANDLE) -> Result<Vec<Entry>> {
    Ok(list_dir_raw(h)?
        .into_iter()
        .map(|e| {
            let is_dir = e.attrs & ATTR_DIRECTORY != 0;
            Entry {
                skip: if e.valid_name {
                    skip_reason(e.attrs, is_dir).map(str::to_owned)
                } else {
                    Some("name is not valid Unicode".to_owned())
                },
                name: Name {
                    long: e.long,
                    short: e.short,
                },
                is_dir,
                link: e.attrs & ATTR_REPARSE_POINT != 0,
            }
        })
        .collect())
}

/// One entry of a directory listing, as the directory reports it.
struct RawEntry {
    long: String,
    /// `long` is the name itself, not a lossy rendering of one that is
    /// not valid Unicode.
    valid_name: bool,
    short: Option<String>,
    attrs: u32,
    /// The 64-bit file reference.
    file_id: i64,
}

fn list_dir_raw(h: HANDLE) -> Result<Vec<RawEntry>> {
    // `u64` elements: the records are 8-byte aligned.
    let mut buf = vec![0u64; 8192];
    let bytes = (buf.len() * size_of::<u64>()) as u32;
    let name_at = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
    let mut class = FileIdBothDirectoryRestartInfo;
    let mut out = Vec::new();
    loop {
        let r = unsafe { GetFileInformationByHandleEx(h, class, buf.as_mut_ptr().cast(), bytes) };
        if let Err(e) = r {
            if e.code() == ERROR_NO_MORE_FILES.into() {
                break;
            }
            return Err(e).context("list directory");
        }
        class = FileIdBothDirectoryInfo;
        let base = buf.as_ptr() as *const u8;
        let mut off = 0usize;
        loop {
            // SAFETY: the call filled the buffer with records linked
            // by `NextEntryOffset`, each within the buffer.
            let info = unsafe { &*(base.add(off) as *const FILE_ID_BOTH_DIR_INFO) };
            let wide = |ptr: *const u16, len: usize| unsafe { std::slice::from_raw_parts(ptr, len) };
            let long = wide(
                unsafe { base.add(off + name_at) } as *const u16,
                info.FileNameLength as usize / 2,
            );
            let short_len = (info.ShortNameLength as u8 as usize / 2).min(info.ShortName.len());
            let short = &info.ShortName[..short_len];
            let dots = long == [u16::from(b'.')] || long == [u16::from(b'.'), u16::from(b'.')];
            if !dots {
                let name = String::from_utf16(long);
                out.push(RawEntry {
                    valid_name: name.is_ok(),
                    long: name.unwrap_or_else(|_| String::from_utf16_lossy(long)),
                    short: String::from_utf16(short).ok().filter(|s| !s.is_empty()),
                    attrs: info.FileAttributes,
                    file_id: info.FileId,
                });
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            off += info.NextEntryOffset as usize;
        }
    }
    Ok(out)
}

/// The profile on disk, for [`plan::plan`]. A directory is listed
/// only after it was opened without following a link, found to be a
/// plain directory, and found to be where its path says.
struct DiskTree<'a> {
    profile: &'a str,
}

/// The object `path` names, with every link on the way resolved - one
/// link at a time, each from the link's own data, and only for as
/// long as the way stays on drive letters of this machine.
fn resolve_on_local_drives(path: &str) -> Resolved {
    const MAX_LINKS: usize = 32;
    let mut cur = path.to_owned();
    for _ in 0..MAX_LINKS {
        if !path_id::is_drive_path(&cur) || path_id::on_network_drive(&cur) {
            return Resolved::Elsewhere(format!("leads to '{cur}', which is not on a local drive; not followed"));
        }
        let (root, tail) = cur.split_at(7);
        let comps: Vec<&str> = tail.split('\\').filter(|c| !c.is_empty()).collect();
        let mut link = None;
        // The last component that was opened, for its spelling on disk.
        let mut above: Option<Opened> = None;
        for i in 0..comps.len() {
            let prefix = format!("{root}{}", comps[..=i].join("\\"));
            let o = match open_no_follow(&prefix, FILE_READ_ATTRIBUTES) {
                Ok(o) => o,
                // This component cannot be opened: it is not there
                // (yet), or the caller may not look.
                Err(_) => {
                    let base = above
                        .as_ref()
                        .and_then(|a| path_id::final_path_of(a.h.raw()).ok())
                        .unwrap_or_else(|| format!("{root}{}", comps[..i].join("\\")));
                    let base = base.trim_end_matches('\\');
                    return Resolved::At(format!("{base}\\{}", comps[i..].join("\\")));
                }
            };
            if o.attrs & ATTR_REPARSE_POINT != 0 {
                let rest: Vec<String> = comps[i + 1..].iter().map(|c| (*c).to_owned()).collect();
                link = Some((prefix, o, rest));
                break;
            }
            if i + 1 == comps.len() {
                return Resolved::At(path_id::final_path_of(o.h.raw()).unwrap_or(prefix));
            }
            above = Some(o);
        }
        let Some((at, o, rest)) = link else {
            // The root of a drive.
            return Resolved::At(cur);
        };
        // A link whose data cannot be read or decoded is skipped, like
        // one that leads out of the profile.
        let data = match read_reparse(o.h.raw()) {
            Ok(data) => data,
            Err(e) => return Resolved::Elsewhere(format!("'{at}': {e:#}")),
        };
        cur = match plan::link_destination(&at, &data, &rest) {
            Ok(next) => next,
            Err(plan::NoDestination::Elsewhere(why) | plan::NoDestination::Malformed(why)) => {
                return Resolved::Elsewhere(why);
            }
        };
    }
    Resolved::Elsewhere(format!("'{path}' leads through more than {MAX_LINKS} links"))
}

/// What [`resolve_on_local_drives`] found.
enum Resolved {
    /// The object, or - where a component on the way cannot be opened -
    /// the path the links lead to as written.
    At(String),
    /// Off this machine's drives, not a redirection, or not decodable:
    /// not followed.
    Elsewhere(String),
}

/// The reparse data of the link open on `h`.
fn read_reparse(h: HANDLE) -> Result<plan::LinkData> {
    const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00A8;
    // MAXIMUM_REPARSE_DATA_BUFFER_SIZE.
    let mut buf = vec![0u8; 16 * 1024];
    let mut returned = 0u32;
    // SAFETY: `buf` is writable for the length passed and outlives
    // the call; `returned` receives how much of it was filled.
    unsafe {
        DeviceIoControl(
            h,
            FSCTL_GET_REPARSE_POINT,
            None,
            0,
            Some(buf.as_mut_ptr().cast()),
            buf.len() as u32,
            Some(&mut returned),
            None,
        )
    }
    .context("read the link")?;
    buf.truncate(returned as usize);
    plan::parse_reparse(&buf).map_err(|e| FenceHelperError::msg((e).to_string()))
}

impl Tree for DiskTree<'_> {
    /// Where the link at `rel` leads, placed against the profile. Only
    /// read; nothing is written at either end here.
    fn resolve_link(&mut self, rel: &[String]) -> LinkTarget {
        match resolve_on_local_drives(&join(self.profile, rel)) {
            Resolved::At(real) => match plan::rel_components(self.profile, &real) {
                Ok(inside) => LinkTarget::Inside(inside),
                Err(_) => LinkTarget::Elsewhere(format!("leads to '{real}'")),
            },
            Resolved::Elsewhere(why) => LinkTarget::Elsewhere(why),
        }
    }

    fn list(&mut self, rel: &[String]) -> std::result::Result<Vec<Entry>, String> {
        let path = join(self.profile, rel);
        (|| -> Result<Vec<Entry>> {
            let o = open_no_follow(&path, ACCESS_LIST)?;
            if let Some(why) = skip_reason(o.attrs, o.is_dir) {
                return Err(FenceHelperError::msg(why.to_string()));
            }
            if !o.is_dir {
                return Err(FenceHelperError::msg("not a directory".to_string()));
            }
            verify_location(&o, &path)?;
            list_dir(o.h.raw())
        })()
        .map_err(|e| format!("{e:#}"))
    }
}

// ─── Record ─────────────────────────────────────────────────────────

const VAL_HEADER: &str = "Profile";
const VAL_EXTRA: &str = "ProfileExtra";
const VAL_ENV_EXTRA: &str = "ProfileEnvExclusions";
const VAL_PATHS: &str = "ProfilePaths";
const VAL_EXCLUSIONS: &str = "ProfileExclusions";
const VAL_UNGRANTED: &str = "ProfileUngranted";
const VAL_NOTES: &str = "ProfileNotes";

/// What a run of the grant left as it found it, and why - the objects
/// in a real profile that its user may not change, or not even open.
/// None of them makes the grant incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteClass {
    /// Was to be granted and could not be written: left unreadable to
    /// sandboxed commands. Nothing became readable through the grant.
    NotGranted,
    /// An exclusion whose deny could not be written, and whose own
    /// access list gives the sandbox account no read.
    SelfClosed,
    /// An exclusion whose deny could not be written and whose access
    /// list could not be read either.
    Unverifiable,
    /// An exclusion whose deny could not be written and whose own
    /// access list lets the sandbox account read it.
    Unprotected,
    /// Carries the allow of an earlier run though it is no longer to
    /// be readable, and the allow could not be removed. Not on the
    /// exclusion list: it has merely left the grant.
    StaleAllow,
    /// An allow of an earlier run that could not be removed, on a path that IS
    /// on the exclusion list now. Reported with the unprotected ones.
    OurAllow,
    /// A directory granted whole that must now be granted child by child
    /// (something beneath it became an exclusion) and could not be rewritten:
    /// the exclusion is readable through the directory's entry.
    OurAllowAbove,
}

impl NoteClass {
    pub const ALL: [NoteClass; 5] = [
        NoteClass::NotGranted,
        NoteClass::SelfClosed,
        NoteClass::Unverifiable,
        NoteClass::Unprotected,
        NoteClass::StaleAllow,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            NoteClass::NotGranted => "not_granted",
            NoteClass::SelfClosed => "self_closed",
            NoteClass::Unverifiable => "unverifiable",
            NoteClass::Unprotected => "unprotected",
            NoteClass::StaleAllow => "stale_allow",
            NoteClass::OurAllow => "our_allow",
            NoteClass::OurAllowAbove => "our_allow_above",
        }
    }

    /// Every class a record can carry: those reported, and the two
    /// that are reported as `unprotected`.
    const RECORDED: [NoteClass; 7] = [
        NoteClass::NotGranted,
        NoteClass::SelfClosed,
        NoteClass::Unverifiable,
        NoteClass::Unprotected,
        NoteClass::StaleAllow,
        NoteClass::OurAllow,
        NoteClass::OurAllowAbove,
    ];
}

/// One such object, as recorded with the grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub class: NoteClass,
    pub path: String,
    pub reason: String,
}

impl Note {
    /// `<class>|<path>|<reason>`; a path has no `|`.
    fn encode(&self) -> String {
        format!("{}|{}|{}", self.class.as_str(), self.path, self.reason)
    }

    fn decode(s: &str) -> Option<Note> {
        let mut it = s.splitn(3, '|');
        let class = it.next()?;
        Some(Note {
            class: NoteClass::RECORDED.into_iter().find(|c| c.as_str() == class)?,
            path: it.next()?.to_owned(),
            reason: it.next().unwrap_or_default().to_owned(),
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn file_id_from_hex(s: &str) -> Result<FileId> {
    if s.len() != 48 || !s.is_ascii() {
        return Err(FenceHelperError::msg(format!("malformed file id '{s}'")));
    }
    let bytes = (0..24)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .with_context(|| format!("malformed file id '{s}'"))?;
    FileId::from_bytes(&bytes)
}

/// What a grant was made for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub list_version: u32,
    /// False from before the first ACE of a run until its end, and
    /// when an exclusion could not be written.
    pub complete: bool,
    pub file_id: FileId,
    pub profile: String,
}

impl Header {
    /// `1|<list version>|<complete>|<file id>|<profile path>`.
    fn encode(&self) -> String {
        format!(
            "1|{}|{}|{}|{}",
            self.list_version,
            u8::from(self.complete),
            hex(&self.file_id.as_bytes()),
            self.profile
        )
    }

    fn decode(s: &str) -> Result<Header> {
        let bad = || FenceHelperError::msg(format!("malformed profile-grant record '{s}'"));
        let mut it = s.splitn(5, '|');
        let mut next = || it.next().ok_or_else(bad);
        if next()? != "1" {
            return Err(bad());
        }
        let list_version = next()?.parse().map_err(|_| bad())?;
        let complete = match next()? {
            "0" => false,
            "1" => true,
            _ => return Err(bad()),
        };
        let file_id = file_id_from_hex(next()?)?;
        let profile = next()?.to_owned();
        if profile.is_empty() {
            return Err(bad());
        }
        Ok(Header {
            list_version,
            complete,
            file_id,
            profile,
        })
    }
}

/// One recorded path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub path: String,
    pub kind: Kind,
    /// A directory the grant created empty.
    pub created: bool,
    /// `None` between the record and the first open of the object.
    pub file_id: Option<FileId>,
}

impl Recorded {
    /// `<file id or ->|<kind>|<created>|<path>`.
    fn encode(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.file_id
                .map(|id| hex(&id.as_bytes()))
                .unwrap_or_else(|| "-".to_owned()),
            self.kind.as_str(),
            u8::from(self.created),
            self.path
        )
    }

    fn decode(s: &str) -> Result<Recorded> {
        let bad = || FenceHelperError::msg(format!("malformed profile-grant entry '{s}'"));
        let mut it = s.splitn(4, '|');
        let mut next = || it.next().ok_or_else(bad);
        let file_id = match next()? {
            "-" => None,
            h => Some(file_id_from_hex(h)?),
        };
        let kind = Kind::parse(next()?).ok_or_else(bad)?;
        let created = match next()? {
            "0" => false,
            "1" => true,
            _ => return Err(bad()),
        };
        let path = next()?.to_owned();
        if path.is_empty() {
            return Err(bad());
        }
        Ok(Recorded {
            path,
            kind,
            created,
            file_id,
        })
    }
}

/// The registry half of the record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marker {
    pub header: Option<Header>,
    /// Exclusions added to the built-in list when the grant was made,
    /// as paths beneath the profile. A later run keeps them unless it
    /// is told to replace them.
    pub extra: Vec<String>,
    /// Exclusions from the calling user's environment ([`plan::ENV_STORES`]).
    /// No run drops them, not even one told to replace the added exclusions:
    /// what was protected once stays protected until the grant is forgotten.
    pub env_extra: Vec<String>,
    /// Every PROTECTED exclusion the grant was made with, as absolute paths,
    /// wildcards as written, existing or not: what must never be granted, by
    /// this grant or by a session on top of it.
    pub exclusions: Vec<String>,
    /// The UNGRANTED-ONLY exclusions, in the same form: directories
    /// the grant leaves unreadable without denying them.
    pub ungranted_only: Vec<String>,
    pub paths: Vec<Recorded>,
    /// What the last run left as it found it ([`NoteClass`]).
    pub notes: Vec<Note>,
}

impl Marker {
    fn is_empty(&self) -> bool {
        self.header.is_none() && self.paths.is_empty()
    }
}

/// The two stores a grant is recorded in. The registry key is a
/// parameter so a test can keep its record apart from the user's.
#[derive(Clone, Copy)]
pub struct Store<'a> {
    pub conn: &'a Connection,
    pub key: &'a str,
}

impl<'a> Store<'a> {
    /// The calling user's record.
    pub fn of(conn: &'a Connection) -> Store<'a> {
        Store {
            conn,
            key: persist::MARKER_KEY,
        }
    }
}

/// Read the marker under `key`. An absent key or value is the empty
/// marker; anything unreadable is an error, which every caller treats
/// as "a grant may exist".
pub fn read_marker_at(key: &str) -> Result<Marker> {
    let Some(k) = reg::Key::open_hkcu(key, false)? else {
        return Ok(Marker::default());
    };
    Ok(Marker {
        header: k.get_sz(VAL_HEADER)?.map(|s| Header::decode(&s)).transpose()?,
        extra: k.get_multi_sz(VAL_EXTRA)?.unwrap_or_default(),
        env_extra: k.get_multi_sz(VAL_ENV_EXTRA)?.unwrap_or_default(),
        exclusions: k.get_multi_sz(VAL_EXCLUSIONS)?.unwrap_or_default(),
        ungranted_only: k.get_multi_sz(VAL_UNGRANTED)?.unwrap_or_default(),
        paths: k
            .get_multi_sz(VAL_PATHS)?
            .unwrap_or_default()
            .iter()
            .map(|s| Recorded::decode(s))
            .collect::<Result<_>>()?,
        // Advisory: an entry that does not parse is dropped, not an
        // error that makes the record unreadable.
        notes: k
            .get_multi_sz(VAL_NOTES)?
            .unwrap_or_default()
            .iter()
            .filter_map(|s| Note::decode(s))
            .collect(),
    })
}

fn write_marker(key: &str, m: &Marker) -> Result<()> {
    let k = reg::Key::create_hkcu(key)?;
    // The paths first: a marker with paths and no header still reads
    // as "a grant may exist".
    let paths: Vec<String> = m.paths.iter().map(Recorded::encode).collect();
    k.set_multi_sz(VAL_PATHS, &paths)?;
    k.set_multi_sz(VAL_EXTRA, &m.extra)?;
    k.set_multi_sz(VAL_ENV_EXTRA, &m.env_extra)?;
    k.set_multi_sz(VAL_EXCLUSIONS, &m.exclusions)?;
    k.set_multi_sz(VAL_UNGRANTED, &m.ungranted_only)?;
    let notes: Vec<String> = m.notes.iter().map(Note::encode).collect();
    k.set_multi_sz(VAL_NOTES, &notes)?;
    match &m.header {
        Some(h) => k.set_sz(VAL_HEADER, &h.encode()),
        None => reg::delete_value(&k, VAL_HEADER),
    }
}

fn clear_marker(key: &str) -> Result<()> {
    if let Some(k) = reg::Key::open_hkcu(key, true)? {
        for v in [
            VAL_HEADER,
            VAL_EXTRA,
            VAL_ENV_EXTRA,
            VAL_EXCLUSIONS,
            VAL_UNGRANTED,
            VAL_PATHS,
            VAL_NOTES,
        ] {
            reg::delete_value(&k, v)?;
        }
    }
    Ok(())
}

/// Whether the session database has the grant's table and a row in
/// it. A database from before the table existed has neither.
pub(crate) fn db_has_rows(conn: &Connection) -> Result<bool> {
    let table: bool = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'profile_paths'")?
        .exists([])?;
    if !table {
        return Ok(false);
    }
    Ok(conn.prepare("SELECT 1 FROM profile_paths LIMIT 1")?.exists([])?)
}

/// Whether a grant is recorded for the calling user, in either store.
pub(crate) fn recorded(conn: &Connection) -> Result<bool> {
    Ok(db_has_rows(conn)? || !read_marker_at(persist::MARKER_KEY)?.is_empty())
}

/// What the grant recorded for `canon`, if the record is for the
/// object `cur`. Read by the converge of [`crate::persist`].
pub(crate) fn recorded_kind(conn: &Connection, canon: &str, cur: FileId) -> Result<Option<Kind>> {
    let row: Option<(String, Vec<u8>)> = conn
        .prepare_cached("SELECT kind, file_id FROM profile_paths WHERE canonical_path = ?1")?
        .query_row(params![canon], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .context("SELECT profile_paths")?;
    Ok(row
        .filter(|(_, id)| id.as_slice() == cur.as_bytes())
        .and_then(|(kind, _)| Kind::parse(&kind)))
}

fn db_rows(conn: &Connection) -> Result<Vec<Recorded>> {
    let mut s = conn.prepare("SELECT canonical_path, file_id, kind, created FROM profile_paths")?;
    let rows = s.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Vec<u8>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (path, id, kind, created) = row?;
        out.push(Recorded {
            kind: Kind::parse(&kind)
                .ok_or_else(|| FenceHelperError::msg(format!("profile_paths holds kind '{kind}' for '{path}'")))?,
            created: created != 0,
            file_id: FileId::from_bytes(&id).ok(),
            path,
        });
    }
    Ok(out)
}

fn upsert_row(conn: &Connection, path: &str, id: Option<FileId>, kind: Kind, created: bool) -> Result<()> {
    let id = id.map(|i| i.as_bytes().to_vec()).unwrap_or_default();
    conn.execute(
        "INSERT INTO profile_paths (canonical_path, file_id, kind, created) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(canonical_path) DO UPDATE SET \
           file_id = excluded.file_id, kind = excluded.kind, \
           created = MAX(profile_paths.created, excluded.created)",
        params![path, id, kind.as_str(), i64::from(created)],
    )
    .context("UPSERT profile_paths")?;
    Ok(())
}

/// The recorded paths of both stores, the database's entry first
/// where both have one (it is the newer).
fn all_recorded(store: &Store<'_>, marker: &Marker) -> Result<Vec<Recorded>> {
    let mut out = db_rows(store.conn)?;
    let mut seen: HashSet<String> = out.iter().map(|r| r.path.to_lowercase()).collect();
    for r in &marker.paths {
        if seen.insert(r.path.to_lowercase()) {
            out.push(r.clone());
        }
    }
    Ok(out)
}

// ─── Running without the write-restricted token ─────────────────────

/// Why a command that runs the sandbox account under the ordinary
/// token must not run now, as far as this grant goes; `None` when it
/// may.
pub fn block_reason() -> Option<String> {
    block_reason_from(read_marker_at(persist::MARKER_KEY)).or_else(|| {
        let conn = crate::state_db::open_db_ro().ok()??;
        db_has_rows(&conn).unwrap_or(true).then(|| {
            "the state database records paths of a profile read grant that the registry \
             no longer lists"
                .to_owned()
        })
    })
}

fn block_reason_from(marker: Result<Marker>) -> Option<String> {
    match marker {
        Err(e) => Some(format!("the record of the profile read grant cannot be read ({e:#})")),
        Ok(m) if m.is_empty() => None,
        Ok(m) => Some(format!(
            "a profile read grant exists ({} path(s) recorded{})",
            m.paths.len(),
            m.header
                .map(|h| format!(", profile '{}'", h.profile))
                .unwrap_or_default()
        )),
    }
}

/// `{state, reason?}` for `fence-helper user status`: `none`, `present`,
/// or `unknown` when the record cannot be read. Registry only.
pub fn brief_status_json() -> serde_json::Value {
    match read_marker_at(persist::MARKER_KEY) {
        Ok(m) if m.is_empty() => serde_json::json!({ "state": "none" }),
        Ok(_) => serde_json::json!({ "state": "present" }),
        Err(e) => serde_json::json!({ "state": "unknown", "reason": format!("{e:#}") }),
    }
}

// ─── Session grants against the recorded grant ──────────────────────

/// Version of the record's format and of what `profile status`
/// prints about it. A reader that does not know the version it finds
/// must not rely on the lists.
pub const RECORD_VERSION: u32 = 1;

/// `acl grant`'s check: no path a session asks for may reach what a
/// recorded profile read grant excludes. A record that cannot be read, or
/// lists no exclusions, refuses every grant.
pub fn check_session_grants(grants: &[(String, bool)], admitted_raw: &[String]) -> Result<()> {
    let admitted: Vec<String> = admitted_raw
        .iter()
        .map(|a| {
            path_id::canonicalize_path(a)
                .map(|(c, _)| c)
                .unwrap_or_else(|_| a.clone())
        })
        .collect();
    check_grants_against(read_marker_at(persist::MARKER_KEY), grants, &admitted, &entry_names)
}

/// What this build does that a caller built separately may rely on,
/// reported as `capabilities` by `user status` and `profile status`.
pub const CAPABILITIES: &[&str] = &[
    "profile-deny-expansion",
    "profile-session-grant-check",
    "kept-grant-holders",
    "read-deny-except",
    "exec-holder",
];

/// The recorded paths that carry an allow of their own, for working
/// out what a session's read denies are also written on
/// ([`plan::RecordedAllows::covered`]).
pub fn recorded_allows() -> Result<plan::RecordedAllows> {
    let m = read_marker_at(persist::MARKER_KEY).context("read the profile read grant record")?;
    Ok(plan::RecordedAllows::new(
        m.paths.iter().map(|r| (r.path.as_str(), r.kind)),
    ))
}

/// The entries of the directory `dir`, read through a handle opened
/// without following a link. Only a path on a drive letter of this
/// machine is opened.
fn entry_names(dir: &str) -> plan::Listing {
    if !path_id::is_drive_path(dir) || path_id::on_network_drive(dir) {
        return Listing::Failed("not on a local drive, and not examined".to_owned());
    }
    let o = match open_no_follow(dir, ACCESS_LIST) {
        Ok(o) => o,
        Err(e) if path_id::is_not_found(&e) => return Listing::Nothing,
        Err(e) => return Listing::Failed(format!("{e:#}")),
    };
    if skip_reason(o.attrs, o.is_dir).is_some() {
        return Listing::Link;
    }
    if !o.is_dir {
        return Listing::Nothing;
    }
    match list_dir(o.h.raw()) {
        Ok(entries) => Listing::Entries(entries.into_iter().map(|e| e.name.long).collect()),
        Err(e) => Listing::Failed(format!("{e:#}")),
    }
}

fn check_grants_against(
    marker: Result<Marker>,
    grants: &[(String, bool)],
    admitted: &[String],
    list: &dyn Fn(&str) -> plan::Listing,
) -> Result<()> {
    let m = marker.map_err(|e| {
        FenceHelperError::msg(format!(
            "the record of the profile read grant cannot be read ({e:#}), so what it \
             excludes is unknown; nothing is granted"
        ))
    })?;
    if m.is_empty() {
        return Ok(());
    }
    if m.exclusions.is_empty() {
        return Err(FenceHelperError::msg(
            "a profile read grant is recorded without its exclusion list; nothing is \
             granted until `fence-helper profile grant` has run again (or `fence-helper profile \
             forget`)"
                .to_string(),
        ));
    }
    for (path, write) in grants {
        if let Some(why) = plan::session_grant_refusal(path, *write, &m.exclusions, &m.ungranted_only, admitted, list) {
            return Err(FenceHelperError::msg(format!(
                "refused, and nothing was granted: {why}"
            )));
        }
    }
    Ok(())
}

/// The SID of the user who owns the interactive session this process runs
/// in, or `None` (session 0, a service). A process started with other
/// credentials in that session has a different token user.
pub fn session_user_sid() -> Option<String> {
    let query = |class: WTS_INFO_CLASS| -> Option<String> {
        let mut buf = windows::core::PWSTR::null();
        let mut bytes = 0u32;
        unsafe { WTSQuerySessionInformationW(None, WTS_CURRENT_SESSION, class, &mut buf, &mut bytes) }.ok()?;
        if buf.is_null() {
            return None;
        }
        let s = unsafe { buf.to_string() }.ok();
        unsafe { WTSFreeMemory(buf.0 as *mut std::ffi::c_void) };
        s.filter(|s| !s.is_empty())
    };
    let user = query(WTSUserName)?;
    let name = match query(WTSDomainName) {
        Some(domain) => format!("{domain}\\{user}"),
        None => user,
    };
    crate::sid::lookup_account_sid(&name).ok()
}

// ─── Grant ──────────────────────────────────────────────────────────

/// A path with why it was skipped or what failed on it.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Item {
    pub path: String,
    pub reason: String,
    /// For an exclusion that could not be examined: whether the
    /// CALLING user can open it for reading - `true`, `false`, or
    /// `"unknown"` where that was not tried.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_reads: Option<serde_json::Value>,
    /// Why it could not be examined: `acl_unreadable`, `account_unknown`,
    /// `not_local` or `probe_failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<&'static str>,
    /// Set where an entry THIS PRODUCT wrote makes the path readable: whether
    /// the path is on the exclusion list now, or has merely left the grant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protected_exclusion: Option<bool>,
    /// For the same: `explicit` (the entry is on the object, which
    /// cannot be written) or `inherited` (a copy handed down from a
    /// directory above, which could not be taken off the object).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<&'static str>,
    /// The path is a directory whose entry exposes an exclusion
    /// BENEATH it, not an exclusion itself.
    #[serde(skip)]
    above: bool,
}

/// What an exclusion without a deny of ours is, with what was found
/// out about it.
struct Probe {
    class: NoteClass,
    reason: String,
    /// `Some` for [`NoteClass::Unverifiable`] only.
    caller_reads: Option<Option<bool>>,
    cause: Option<&'static str>,
    /// `Some` where our own entry is what lets the sandbox account in.
    entry: Option<&'static str>,
}

impl Probe {
    fn settled(class: NoteClass, reason: String) -> Self {
        Probe {
            class,
            reason,
            caller_reads: None,
            cause: None,
            entry: None,
        }
    }

    fn unverifiable(cause: &'static str, caller_reads: Option<bool>, reason: String) -> Self {
        Probe {
            class: NoteClass::Unverifiable,
            reason,
            caller_reads: Some(caller_reads),
            cause: Some(cause),
            entry: None,
        }
    }

    fn item(&self, path: &str) -> Item {
        Item {
            path: path.to_owned(),
            reason: self.reason.clone(),
            caller_reads: self.caller_reads.map(|known| match known {
                Some(reads) => serde_json::Value::Bool(reads),
                None => serde_json::Value::String("unknown".to_owned()),
            }),
            cause: self.cause,
            // Every exclusion judged here is on the exclusion list.
            protected_exclusion: self.entry.map(|_| true),
            entry: self.entry,
            above: false,
        }
    }
}

/// Outcome of [`grant`], and the `--json` output of `profile grant`.
#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub profile: String,
    pub list_version: u32,
    /// Every exclusion that is a directory carries its deny. Only a
    /// complete grant is reported as current by `profile status`.
    pub complete: bool,
    pub containers: usize,
    pub granted: usize,
    pub files: usize,
    pub denied: usize,
    /// Excluded directories created empty by this run.
    pub placeholders: usize,
    /// DACLs written by this run, and paths found as wanted.
    pub writes: usize,
    pub unchanged: usize,
    /// Write roots kept from earlier sessions whose grant was removed by
    /// this run: each is, lies inside, or lies above an exclusion, so its
    /// inheriting allow would have reached one.
    pub forgotten_roots: Vec<Item>,
    /// `VAR=value` settings of the calling user's environment that were not
    /// followed because the value is a relative path.
    pub env_ignored: Vec<String>,
    /// Of this run's DACL writes: those that walked a tree, and those made on
    /// an object alone.
    pub tree_walks: usize,
    pub local_writes: usize,
    /// Recorded paths that are no longer part of the grant and had
    /// their entry removed.
    pub removed: usize,
    pub skipped: Vec<Item>,
    /// Paths that were to be granted and could not be written. They
    /// stay unreadable to sandboxed commands; where one was to be a
    /// container, nothing beneath it is granted either.
    pub not_granted: Vec<Item>,
    /// Exclusions whose deny could not be written: closed by their own access
    /// list; with a list that could not be read; readable by their own list.
    pub self_closed: Vec<Item>,
    pub unverifiable: Vec<Item>,
    pub unprotected: Vec<Item>,
    /// Paths that have left the grant and still carry the allow of an
    /// earlier run, which could not be removed: readable through an
    /// entry of ours on an object that cannot be written.
    pub stale_allow: Vec<Item>,
    pub elapsed_ms: u64,
}

impl Report {
    fn note(&mut self, class: NoteClass, path: &str, reason: String) {
        self.note_item(
            class,
            Item {
                path: path.to_owned(),
                reason,
                ..Default::default()
            },
        )
    }

    fn note_item(&mut self, class: NoteClass, item: Item) {
        match class {
            NoteClass::NotGranted => self.not_granted.push(item),
            NoteClass::SelfClosed => self.self_closed.push(item),
            NoteClass::Unverifiable => self.unverifiable.push(item),
            NoteClass::Unprotected | NoteClass::OurAllow | NoteClass::OurAllowAbove => self.unprotected.push(item),
            NoteClass::StaleAllow => self.stale_allow.push(item),
        }
    }

    fn notes(&self) -> Vec<Note> {
        let of = |class: NoteClass, items: &[Item]| {
            items
                .iter()
                .map(|i| Note {
                    // Open through an entry of ours: recorded as
                    // that, on the object or on a directory above.
                    class: match (class, i.entry, i.above) {
                        (NoteClass::Unprotected, Some(_), false) => NoteClass::OurAllow,
                        (NoteClass::Unprotected, Some(_), true) => NoteClass::OurAllowAbove,
                        _ => class,
                    },
                    path: i.path.clone(),
                    reason: i.reason.clone(),
                })
                .collect::<Vec<_>>()
        };
        [
            of(NoteClass::NotGranted, &self.not_granted),
            of(NoteClass::SelfClosed, &self.self_closed),
            of(NoteClass::Unverifiable, &self.unverifiable),
            of(NoteClass::Unprotected, &self.unprotected),
            of(NoteClass::StaleAllow, &self.stale_allow),
        ]
        .concat()
    }
}

/// The SIDs of the sandbox account's token: those it always carries.
fn sandbox_sids(ids: &Ids) -> Vec<&str> {
    ids.sandbox_user_sid
        .as_deref()
        .into_iter()
        .chain(ids.group_sid.as_deref())
        .chain(plan::SANDBOX_ALWAYS)
        .collect()
}

/// What the object's own access list lets the sandbox account read, on it
/// or in what it hands down; `Ok(None)` if nothing is there. Read with
/// `READ_CONTROL` alone ([`open_bare`]).
fn own_access(path: &str, ids: &Ids) -> std::result::Result<Option<Access>, (&'static str, String)> {
    let failed = |e: &FenceHelperError| {
        let cause = if is_access_denied(e) {
            "acl_unreadable"
        } else {
            "probe_failed"
        };
        (cause, format!("{e:#}"))
    };
    let h = match open_bare(path, READ_CONTROL) {
        Ok(h) => h,
        Err(e) if path_id::is_not_found(&e) => return Ok(None),
        Err(e) => return Err(failed(&e)),
    };
    let dacl = acl::read_dacl_views(acl::DaclTarget::Handle(h.raw(), path)).map_err(|e| failed(&e))?;
    let Some(aces) = dacl else {
        // No access list at all: everyone reads.
        return Ok(Some(Access {
            reads: true,
            ours: None,
        }));
    };
    let facts: Vec<plan::AceFact<'_>> = aces
        .iter()
        .map(|a| plan::AceFact {
            deny: a.deny,
            sid: &a.sid,
            mask: a.mask,
            inherit_only: a.flags & 0x8 != 0,
            // Object-inherit or container-inherit.
            inherited_beneath: a.flags & 0x3 != 0,
            inherited: a.inherited,
        })
        .collect();
    let by = plan::reading_entry(&facts, &sandbox_sids(ids), &plan::SANDBOX_MAYBE);
    // An entry for the sandbox group - this install's, or an earlier
    // one's - is an entry this product wrote.
    let is_ours = |sid: &str| ids.group_sid.as_deref() == Some(sid) || ids.previous_group_sids.iter().any(|g| g == sid);
    Ok(Some(Access {
        reads: by.is_some(),
        ours: by
            .filter(|a| is_ours(a.sid))
            .map(|a| if a.inherited { "inherited" } else { "explicit" }),
    }))
}

/// What an object's own access list says about the sandbox account.
struct Access {
    reads: bool,
    /// The entry that lets it read is one this product wrote: set on
    /// the object (`explicit`) or a copy from above (`inherited`).
    ours: Option<&'static str>,
}

/// The line for a path that sandboxed commands can read through an
/// entry this product wrote and could not take back, with what its
/// user can do about it.
fn our_entry_line(path: &str, ids: &Ids, entry: &str) -> String {
    let shown = path_id::strip_extended_prefix(path);
    let group = ids.group_sid.as_deref().unwrap_or("<sandbox group SID>");
    if entry == "inherited" {
        format!(
            "it is readable to sandboxed commands through a copy of an entry this product \
             wrote on a directory above it; the copy could not be taken off this object. \
             As its owner, or from an elevated prompt: icacls \"{shown}\" /inheritance:e - \
             that re-derives its inherited entries - then run the grant again"
        )
    } else {
        format!(
            "it is readable to sandboxed commands through an entry this product wrote on \
             it and could not remove. As its owner, or from an elevated prompt: icacls \
             \"{shown}\" /remove:g \"*{group}\" - then run the grant again"
        )
    }
}

/// An excluded path whose deny could not be written (`why`): which of
/// the three it is ([`plan::Unstamped`]), with the line for the
/// report.
fn exclusion_without_deny(path: &str, ids: &Ids, why: &FenceHelperError) -> Probe {
    exclusion_as_it_is(path, ids, &format!("could not be written ({why:#})"))
}

/// What an exclusion that carries no deny of ours is NOW, by its own
/// access list. A look that fails is `Unverifiable`, never closed.
fn exclusion_as_it_is(path: &str, ids: &Ids, why: &str) -> Probe {
    if !path_id::is_drive_path(path) || path_id::on_network_drive(path) {
        return Probe::unverifiable(
            "not_local",
            None,
            format!("its deny {why}, and it is not on a local drive, so it is not examined"),
        );
    }
    match own_access(path, ids) {
        Ok(None) => Probe::settled(
            NoteClass::SelfClosed,
            format!("nothing is there to deny (its deny {why})"),
        ),
        // Said of the object and of what it hands down, not of every
        // object already inside it: an excluded store is not walked.
        Ok(Some(Access { reads: false, .. })) => Probe::settled(
            NoteClass::SelfClosed,
            format!(
                "its deny {why}; its own access list gives the sandbox account no read, \
                 on it or handed down beneath it"
            ),
        ),
        // Open through an entry of OURS: the strongest case.
        Ok(Some(Access {
            reads: true,
            ours: Some(entry),
        })) => Probe {
            entry: Some(entry),
            ..Probe::settled(
                NoteClass::Unprotected,
                format!("its deny {why}, and {}", our_entry_line(path, ids, entry)),
            )
        },
        Ok(Some(Access { reads: true, .. })) => Probe::settled(
            NoteClass::Unprotected,
            format!(
                "its deny {why}, and its own access list lets the sandbox account read it \
                 or what is put into it - the grant did not open it and cannot close it. \
                 Only its owner or an administrator can"
            ),
        ),
        Err((cause, e)) => {
            const FILE_READ_DATA: u32 = 0x1;
            let caller_reads = open_bare(path, FILE_READ_DATA).is_ok();
            Probe::unverifiable(
                cause,
                Some(caller_reads),
                format!(
                    "its deny {why} and its access list could not be read ({e}); {}",
                    if caller_reads {
                        "you can read its content all the same"
                    } else {
                        "you cannot read its content either"
                    }
                ),
            )
        }
    }
}

/// Whether `path` is closed to the sandbox account by its own access list,
/// so that a session read deny that cannot be written there is already
/// satisfied. `false` whenever that cannot be established.
pub fn closed_to_sandbox(path: &str) -> bool {
    let Ok(ids) = Ids::load() else {
        return false;
    };
    let access = own_access(path, &ids);
    matches!(access, Ok(Some(Access { reads: false, .. })))
}

/// Whether the object at `path` carries no entry of ours, read with
/// the least access that can. `false` whenever that cannot be
/// established.
fn nothing_of_ours_on(path: &str, ids: &Ids) -> bool {
    let Ok(h) = open_bare(path, READ_CONTROL) else {
        return false;
    };
    acl::read_aces_at(acl::DaclTarget::Handle(h.raw(), path)).is_ok_and(|aces| {
        !aces
            .iter()
            .any(|a| !a.inherited && acl::is_persistent_ace(a, &ids.trustees()))
    })
}

enum Stamped {
    Wrote,
    Unchanged,
    Skipped(&'static str),
}

/// Open the node without following a link, check it is still what
/// the plan saw, record it, and converge its ACEs through the handle.
fn stamp(store: &Store<'_>, ids: &Ids, path: &str, node: &Node, created: bool) -> Result<Stamped> {
    let o = open_for_stamp(path, Some(node.is_dir))?;
    if let Some(why) = skip_reason(o.attrs, o.is_dir) {
        return Ok(Stamped::Skipped(why));
    }
    if o.is_dir != node.is_dir {
        return Err(FenceHelperError::msg(format!(
            "is no longer a {}",
            if node.is_dir { "directory" } else { "file" }
        )));
    }
    verify_location(&o, path)?;
    upsert_row(store.conn, path, Some(o.id), node.kind, created)?;
    let obj = o.object();
    match persist::converge_on(store.conn, ids, path, Some(&obj)) {
        Ok(true) => Ok(Stamped::Wrote),
        Ok(false) => Ok(Stamped::Unchanged),
        Err(e) => {
            if persist::settled_on_object(store.conn, ids, path, &obj).unwrap_or(false) {
                eprintln!(
                    "fence-helper: profile grant: '{path}' carries its entry; not every object \
                     below it could be updated ({e:#})"
                );
                Ok(Stamped::Wrote)
            } else {
                Err(e)
            }
        }
    }
}

/// Take a recorded path out of the grant: remove its row, and the
/// group's ACE from the recorded object - at its path, or wherever
/// its file id is now. `Ok(true)` if an ACE was removed.
fn strip(store: &Store<'_>, ids: &Ids, r: &Recorded) -> Result<bool> {
    // The open error decides (`path_id::classify_open_failure`).
    let mut unavailable = false;
    let is_dir = match r.kind {
        Kind::Container | Kind::Granted => Some(true),
        Kind::File => Some(false),
        _ => None,
    };
    let here = match open_for_stamp(&r.path, is_dir) {
        Ok(o) => Some(o),
        Err(e) => match path_id::open_failure(&e) {
            path_id::OpenFailure::NotFound => None,
            path_id::OpenFailure::VolumeUnavailable => {
                unavailable = true;
                None
            }
            // Cannot be written and carries nothing of ours: nothing to
            // remove, not a failure. Otherwise it stays recorded.
            path_id::OpenFailure::Other if is_access_denied(&e) && nothing_of_ours_on(&r.path, ids) => {
                store
                    .conn
                    .execute("DELETE FROM profile_paths WHERE canonical_path = ?1", params![r.path])?;
                return Ok(false);
            }
            path_id::OpenFailure::Other => return Err(e),
        },
    };
    let network = !path_id::is_drive_path(&r.path) || path_id::on_network_drive(&r.path);
    let mut targets: Vec<(String, Opened)> = Vec::new();
    match (here, r.file_id) {
        (Some(o), Some(id)) if o.id == id => targets.push((r.path.clone(), o)),
        // Never opened after it was recorded: the path is all that
        // identifies it.
        (Some(o), None) => targets.push((r.path.clone(), o)),
        (None, None) if unavailable => {
            return Err(FenceHelperError::msg(format!(
                "'{}' is on a volume that cannot be reached now, and nothing records which \
             object it was; its entry may still be on disk",
                r.path
            )));
        }
        (None, None) => {}
        (_, Some(id)) => match path_id::locate_by_file_id(&id) {
            Located::At(p) => {
                let o = open_for_stamp(&p, is_dir)?;
                if o.id == id {
                    targets.push((p, o));
                }
            }
            Located::Gone => {}
            // Its volume is not mounted now: the entry waits with the kept
            // grants whose volume is gone (`persist::retry_pending`).
            Located::VolumeAbsent | Located::Unknown if unavailable => {
                persist::marker_add_pending(&[persist::PendingStrip {
                    file_id: id,
                    group_sid: ids.group_sid.clone().unwrap_or_default(),
                    path: r.path.clone(),
                }])?;
            }
            Located::VolumeAbsent if !network => {
                persist::marker_add_pending(&[persist::PendingStrip {
                    file_id: id,
                    group_sid: ids.group_sid.clone().unwrap_or_default(),
                    path: r.path.clone(),
                }])?;
            }
            Located::VolumeAbsent | Located::Unknown => {
                return Err(FenceHelperError::msg(format!(
                    "the object recorded for '{}' is no longer at that path and could not be \
                 located by file id; its entry may still be on disk",
                    r.path
                )));
            }
        },
    }
    let tx = store.conn.unchecked_transaction().context("begin profile strip")?;
    tx.execute("DELETE FROM profile_paths WHERE canonical_path = ?1", params![r.path])?;
    let mut removed = false;
    for (p, o) in &targets {
        let obj = o.object();
        removed |= match persist::converge_on(&tx, ids, p, Some(&obj)) {
            Ok(w) => w,
            Err(e) => {
                if persist::settled_on_object(&tx, ids, p, &obj).unwrap_or(false) {
                    true
                } else {
                    return Err(e);
                }
            }
        };
    }
    tx.commit().context("commit profile strip")?;
    Ok(removed)
}

/// Exclusions added by the caller, as components beneath the profile,
/// resolved where they exist. One outside the profile is an error.
fn resolve_extra(profile: &str, raw: &[String]) -> Result<Vec<Vec<String>>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for r in raw {
        let absolute = match plan::rel_components(profile, r) {
            Ok(rel) => join(profile, &rel),
            Err(e) => return Err(FenceHelperError::msg(format!("--exclude: {e}"))),
        };
        let resolved = match path_id::canonicalize_path(&absolute) {
            Ok((c, _)) => c,
            Err(_) => absolute,
        };
        let rel =
            plan::rel_components(profile, &resolved).map_err(|e| FenceHelperError::msg(format!("--exclude: {e}")))?;
        if rel.is_empty() {
            return Err(FenceHelperError::msg(format!("--exclude '{r}' is the profile itself")));
        }
        if !out.contains(&rel) {
            out.push(rel);
        }
    }
    Ok(out)
}

/// `profile grant`: bring the profile to the plan. Order: the record
/// (marked incomplete), every exclusion's deny, stale removals, the allows.
/// A path that cannot be written is reported by class, never a failure.
pub fn grant(
    store: &Store<'_>,
    ids: &Ids,
    layout: &Layout,
    extra_raw: &[String],
    replace_extra: bool,
    progress: bool,
) -> Result<Report> {
    let started = std::time::Instant::now();
    let writes_before = acl::write_counts();
    if ids.group_sid.is_none() {
        return Err(FenceHelperError::msg(
            "no install record: run `fence-helper install`".to_string(),
        ));
    }
    let profile = layout.profile.as_str();
    let root = open_for_stamp(profile, None)?;
    if let Some(why) = skip_reason(root.attrs, root.is_dir) {
        return Err(FenceHelperError::msg(format!(
            "the profile directory '{profile}' is a {why}"
        )));
    }
    if !root.is_dir {
        return Err(FenceHelperError::msg(format!("'{profile}' is not a directory")));
    }
    let root_id = root.id;
    drop(root);

    let old = read_marker_at(store.key)?;
    let mut extra: Vec<Vec<String>> = if replace_extra {
        Vec::new()
    } else {
        old.extra
            .iter()
            .map(|x| plan::split_path(x))
            .filter(|x| !x.is_empty())
            .collect()
    };
    for x in resolve_extra(profile, extra_raw)? {
        let same = |a: &Vec<String>| {
            a.len() == x.len()
                && a.iter()
                    .zip(&x)
                    .all(|(p, q)| plan::norm_component(p) == plan::norm_component(q))
        };
        if !extra.iter().any(same) {
            extra.push(x);
        }
    }
    // Stores the caller's environment moves inside the profile, added
    // to those an earlier run recorded; no run drops them.
    let mut env_extra: Vec<Vec<String>> = old
        .env_extra
        .iter()
        .map(|x| plan::split_path(x))
        .filter(|x| !x.is_empty())
        .collect();
    let (named_by_env, env_ignored) = plan::env_exclusions(&|var| layout.own(var));
    for named in named_by_env {
        for x in resolve_extra(profile, &[named]).unwrap_or_default() {
            if !env_extra.contains(&x) {
                env_extra.push(x);
            }
        }
    }
    let env_strings: Vec<String> = env_extra.iter().map(|x| x.join("\\")).collect();
    let rules = Rules::new(
        layout.rel(&layout.roaming).as_deref(),
        layout.rel(&layout.local).as_deref(),
        &[extra.clone(), env_extra].concat(),
    );
    // The rules the plan ends with include those carried over to the
    // targets of links; they are what is recorded.
    let (nodes, rules) = plan::plan_resolved(&mut DiskTree { profile }, &rules)
        .map_err(|e| FenceHelperError::msg(format!("plan the grant on '{profile}': {e}")))?;

    let mut report = Report {
        profile: profile.to_owned(),
        list_version: plan::LIST_VERSION,
        complete: true,
        env_ignored,
        ..Default::default()
    };
    let mut planned: Vec<(String, &Node)> = Vec::new();
    for n in &nodes {
        let path = join(profile, &n.rel);
        match n.kind {
            Kind::Skipped => report.skipped.push(Item {
                path,
                reason: n.note.clone().unwrap_or_default(),
                ..Default::default()
            }),
            // Its owner writes its deny; here it is only left out.
            Kind::Denied if !n.stamp => {}
            // Left exactly as it is: no allow, no deny, no record.
            Kind::Ungranted => {}
            _ => planned.push((path, n)),
        }
    }
    let planned_keys: HashSet<String> = planned.iter().map(|(p, _)| p.to_lowercase()).collect();

    // What an earlier run recorded.
    let before = all_recorded(store, &old)?;
    let is_allow = |k: Kind| matches!(k, Kind::Container | Kind::Granted | Kind::File);
    let earlier = |path: &str| before.iter().find(|r| r.path.eq_ignore_ascii_case(path));

    // The record goes first, marked incomplete, so that a run that is
    // KILLED leaves nothing on disk that `profile forget` does not know.
    if store.key == persist::MARKER_KEY {
        persist::marker_set_ever()?;
        if let Some(g) = &ids.group_sid {
            persist::marker_set_group_sid(g)?;
        }
    }
    let extra_strings: Vec<String> = extra.iter().map(|x| x.join("\\")).collect();
    let absolute = |rel: Vec<String>| -> Vec<String> {
        rel.iter()
            .map(|rel| format!("{}\\{rel}", profile.trim_end_matches('\\')))
            .collect()
    };
    let exclusions = absolute(rules.spelled());
    let ungranted_only = absolute(rules.spelled_ungranted());
    let mut header = Header {
        list_version: plan::LIST_VERSION,
        complete: false,
        file_id: root_id,
        profile: profile.to_owned(),
    };
    let mut pre = before.clone();
    let known: HashSet<String> = pre.iter().map(|r| r.path.to_lowercase()).collect();
    for (path, n) in &planned {
        if !known.contains(&path.to_lowercase()) {
            pre.push(Recorded {
                path: path.clone(),
                kind: n.kind,
                created: n.missing,
                file_id: None,
            });
        }
    }
    write_marker(
        store.key,
        &Marker {
            header: Some(header.clone()),
            extra: extra_strings.clone(),
            env_extra: env_strings.clone(),
            exclusions: exclusions.clone(),
            ungranted_only: ungranted_only.clone(),
            paths: pre,
            notes: old.notes.clone(),
        },
    )?;

    // Excluded directories that are not there yet.
    let mut created_now: HashSet<&str> = HashSet::new();
    for (path, n) in planned.iter().filter(|(_, n)| n.missing) {
        upsert_row(store.conn, path, None, n.kind, true)?;
        match std::fs::create_dir(path) {
            Ok(()) => {
                report.placeholders += 1;
                created_now.insert(path.as_str());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Someone else made it meanwhile: not ours to remove.
                store.conn.execute(
                    "UPDATE profile_paths SET created = 0 WHERE canonical_path = ?1",
                    params![path],
                )?;
            }
            // It cannot be made, so there is nothing there to read
            // either; the stamp below reports it.
            Err(_) => {
                store
                    .conn
                    .execute("DELETE FROM profile_paths WHERE canonical_path = ?1", params![path])?;
            }
        }
    }

    let count = |report: &mut Report, n: &Node, s: &Stamped| {
        match s {
            Stamped::Wrote => report.writes += 1,
            _ => report.unchanged += 1,
        }
        match n.kind {
            Kind::Container => report.containers += 1,
            Kind::Granted => report.granted += 1,
            Kind::File => report.files += 1,
            Kind::Denied => report.denied += 1,
            Kind::Skipped | Kind::Ungranted => {}
        }
    };
    let unrecord = |path: &str| -> Result<()> {
        store
            .conn
            .execute("DELETE FROM profile_paths WHERE canonical_path = ?1", params![path])?;
        Ok(())
    };
    // A path an earlier run wrote on that cannot be written now keeps its
    // entry and stays recorded. Returns `false` if it was never recorded.
    let keeps_earlier =
        |report: &mut Report, path: &str, wanted: Option<&Node>, why: &FenceHelperError| -> Result<bool> {
            let Some(r) = earlier(path) else {
                return Ok(false);
            };
            // The stamp may have got as far as recording the new kind.
            upsert_row(store.conn, &r.path, r.file_id, r.kind, r.created)?;
            match (is_allow(r.kind), wanted.map(|n| is_allow(n.kind))) {
                // An allow of ours stays on a path that is no longer to be readable:
                // on the exclusion list now it is `OurAllow`, otherwise `StaleAllow`.
                (true, Some(false)) | (true, None) => {
                    let excluded_now = wanted.is_some()
                        || plan::rel_components(profile, path).is_ok_and(|rel| {
                            let names: Vec<Name> = rel.iter().map(|c| Name::long(c)).collect();
                            rules.excludes(&names)
                        });
                    let line = our_entry_line(path, ids, "explicit");
                    let item = Item {
                        path: path.to_owned(),
                        reason: if excluded_now {
                            format!("it is excluded now, and {line} ({why:#})")
                        } else {
                            format!("it is no longer part of the grant, and {line} ({why:#})")
                        },
                        protected_exclusion: Some(excluded_now),
                        entry: Some("explicit"),
                        ..Default::default()
                    };
                    report.note_item(
                        if excluded_now {
                            NoteClass::OurAllow
                        } else {
                            NoteClass::StaleAllow
                        },
                        item,
                    );
                }
                // Granted WHOLE earlier and now to be a container: the entry
                // that hands reading down to everything beneath is still there.
                (true, Some(true)) if r.kind == Kind::Granted && wanted.is_some_and(|n| n.kind == Kind::Container) => {
                    let excludes_beneath = plan::rel_components(profile, path).is_ok_and(|rel| {
                        let names: Vec<Name> = rel.iter().map(|c| Name::long(c)).collect();
                        rules.could_exclude_beneath(&names)
                    });
                    let line = our_entry_line(path, ids, "explicit");
                    let item = Item {
                        path: path.to_owned(),
                        reason: if excludes_beneath {
                            format!(
                                "an exclusion lies beneath it, and whatever beneath it carries no \
                             deny of its own is still readable: {line} ({why:#})"
                            )
                        } else {
                            format!(
                                "it is to be granted child by child now and everything beneath \
                             it is still readable: {line} ({why:#})"
                            )
                        },
                        protected_exclusion: Some(excludes_beneath),
                        entry: Some("explicit"),
                        above: true,
                        ..Default::default()
                    };
                    report.note_item(
                        if excludes_beneath {
                            NoteClass::OurAllowAbove
                        } else {
                            NoteClass::StaleAllow
                        },
                        item,
                    );
                }
                (false, Some(true)) => report.note(
                    NoteClass::NotGranted,
                    path,
                    format!(
                        "the deny of an earlier run is on it and could not be replaced \
                         ({why:#}); it stays unreadable to sandboxed commands"
                    ),
                ),
                _ => {
                    report.skipped.push(Item {
                        path: path.to_owned(),
                        reason: format!(
                            "carries its entry from an earlier run and could not be looked at \
                             again ({why:#})"
                        ),
                        ..Default::default()
                    });
                    if let Some(n) = wanted {
                        count(report, n, &Stamped::Unchanged);
                    }
                }
            }
            Ok(true)
        };

    // 0. A write root kept from an earlier session that no session could
    //    be granted now would hand its allow down into an exclusion.
    if store.key == persist::MARKER_KEY {
        for root in persist::kept_roots(store.conn)? {
            let Some(why) = plan::session_grant_refusal(&root, true, &exclusions, &ungranted_only, &[], &entry_names)
            else {
                continue;
            };
            persist::forget_one(store.conn, ids, &root, false).with_context(|| {
                format!(
                    "the write grant kept on '{root}' reaches an exclusion ({why}) and could \
                     not be removed; run `fence-helper acl forget \"{}\"` and then the grant again",
                    path_id::strip_extended_prefix(&root)
                )
            })?;
            report.forgotten_roots.push(Item {
                path: root,
                reason: why,
                ..Default::default()
            });
        }
    }

    // 1. Every exclusion, before any allow.
    let denies: Vec<&(String, &Node)> = planned.iter().filter(|(_, n)| n.kind == Kind::Denied).collect();
    if progress {
        eprintln!(
            "fence-helper: profile grant: {} exclusion(s), then {} path(s) to grant",
            denies.len(),
            planned.len() - denies.len()
        );
    }
    for (path, n) in denies {
        match stamp(store, ids, path, n, created_now.contains(path.as_str())) {
            Ok(Stamped::Skipped(why)) => report.skipped.push(Item {
                path: path.clone(),
                reason: why.to_owned(),
                ..Default::default()
            }),
            Ok(s) => count(&mut report, n, &s),
            Err(e) => {
                if keeps_earlier(&mut report, path, Some(n), &e)? {
                    continue;
                }
                // The deny is the second layer: nothing above an exclusion
                // carries an inheriting allow. Its own access list decides the class.
                unrecord(path)?;
                let probe = exclusion_without_deny(path, ids, &e);
                report.note_item(probe.class, probe.item(path));
            }
        }
    }

    // 2. Entries of an earlier run that the plan no longer has.
    for r in before.iter().filter(|r| !planned_keys.contains(&r.path.to_lowercase())) {
        match strip(store, ids, r) {
            Ok(_) => report.removed += 1,
            Err(e) => {
                keeps_earlier(&mut report, &r.path, None, &e)?;
            }
        }
    }

    // 3. The allows, parents first, one top-level item after another.
    let top_total = {
        let mut tops: Vec<String> = planned
            .iter()
            .filter(|(_, n)| n.kind != Kind::Denied)
            .filter_map(|(_, n)| n.rel.first().map(|c| c.to_lowercase()))
            .collect();
        tops.dedup();
        tops.len()
    };
    let (mut top_done, mut top_now): (usize, Option<String>) = (0, None);
    // Containers that could not be written, as `path\` in lower case:
    // nothing beneath one is newly granted.
    let mut closed: Vec<String> = Vec::new();
    for (path, n) in planned.iter().filter(|(_, n)| n.kind != Kind::Denied) {
        let top = n.rel.first().map(|c| c.to_lowercase());
        if top != top_now {
            if top_now.is_some() {
                top_done += 1;
                if progress {
                    eprintln!("fence-helper: profile grant: {top_done}/{top_total} top-level item(s) done");
                }
            }
            top_now = top;
        }
        let lower = path.to_lowercase();
        if closed.iter().any(|c| lower.starts_with(c.as_str())) {
            if earlier(path).is_none() {
                unrecord(path)?;
            }
            continue;
        }
        match stamp(store, ids, path, n, created_now.contains(path.as_str())) {
            Ok(Stamped::Skipped(why)) => {
                report.skipped.push(Item {
                    path: path.clone(),
                    reason: why.to_owned(),
                    ..Default::default()
                });
                // What it was when an earlier run granted it is found
                // by its identity and cleared, if it can be.
                match earlier(path) {
                    Some(r) => {
                        if let Err(e) = strip(store, ids, r) {
                            keeps_earlier(&mut report, path, None, &e)?;
                        }
                    }
                    None => unrecord(path)?,
                }
            }
            Ok(s) => count(&mut report, n, &s),
            // Not granted: nothing became readable through the grant,
            // so the grant is whole without it.
            Err(e) => {
                if n.rel.is_empty() && earlier(path).is_none() {
                    // The record still says "incomplete"; `profile forget` clears it.
                    return Err(e.context(format!("the profile directory '{profile}' cannot be written")));
                }
                if keeps_earlier(&mut report, path, Some(n), &e)? {
                    continue;
                }
                unrecord(path)?;
                let beneath = if n.kind == Kind::Container {
                    closed.push(format!("{lower}\\"));
                    "; nothing beneath it is granted either"
                } else {
                    ""
                };
                report.note(
                    NoteClass::NotGranted,
                    path,
                    format!("{e:#}; it stays unreadable to sandboxed commands{beneath}"),
                );
            }
        }
    }
    if progress && top_now.is_some() {
        eprintln!(
            "fence-helper: profile grant: {}/{top_total} top-level item(s) done",
            top_done + 1
        );
    }

    // The record as it now is: every row, and nothing else.
    header.complete = true;
    report.complete = true;
    let paths = db_rows(store.conn)?;
    let notes = report.notes();
    write_marker(
        store.key,
        &Marker {
            header: Some(header),
            extra: extra_strings,
            env_extra: env_strings,
            exclusions,
            ungranted_only,
            paths,
            notes,
        },
    )?;
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    let writes_after = acl::write_counts();
    report.tree_walks = writes_after.0 - writes_before.0;
    report.local_writes = writes_after.1 - writes_before.1;
    Ok(report)
}

// ─── Forget ─────────────────────────────────────────────────────────

/// Outcome of [`forget`], and the `--json` output of `profile forget`.
#[derive(Debug, Default, serde::Serialize)]
pub struct Forgotten {
    /// Paths whose entry was removed, and paths that carried none.
    pub removed: usize,
    pub absent: usize,
    /// Empty directories the grant had created, removed again.
    pub placeholders_removed: usize,
    /// Paths that could not be completed; they stay recorded.
    pub failed: Vec<Item>,
}

/// Mark the record incomplete before the first entry comes off: a forget
/// that is killed leaves a grant no session starts on, which the next
/// `profile grant`, `profile forget` or uninstall finishes.
fn mark_forget_begun(key: &str, marker: &Marker) -> Result<()> {
    match &marker.header {
        Some(h) if h.complete => write_marker(
            key,
            &Marker {
                header: Some(Header {
                    complete: false,
                    ..h.clone()
                }),
                ..marker.clone()
            },
        ),
        _ => Ok(()),
    }
}

/// `profile forget`: remove the group's ACE from every recorded
/// object, the empty directories the grant created, and the record.
/// Every DACL is left as it was, minus this grant's entries.
pub fn forget(store: &Store<'_>, ids: &Ids) -> Result<Forgotten> {
    // Without the install record the group's SID comes from the
    // account database: its ACEs must be recognised to be removed.
    let named;
    let ids = if ids.group_sid.is_none() {
        named = Ids {
            group_sid: crate::sid::lookup_account_sid(crate::user::SANDBOX_GROUP).ok(),
            ..ids.clone()
        };
        &named
    } else {
        ids
    };
    let marker = read_marker_at(store.key)?;
    mark_forget_begun(store.key, &marker)?;
    let all = all_recorded(store, &marker)?;
    let mut out = Forgotten::default();
    let mut failed_paths: HashSet<&str> = HashSet::new();
    for r in &all {
        match strip(store, ids, r) {
            Ok(true) => out.removed += 1,
            Ok(false) => out.absent += 1,
            Err(e) => {
                eprintln!("fence-helper: profile forget '{}': {e:#}", r.path);
                failed_paths.insert(&r.path);
                out.failed.push(Item {
                    path: r.path.clone(),
                    reason: format!("{e:#}"),
                    ..Default::default()
                });
            }
        }
    }
    // Directories the grant created, deepest first, if still the same
    // object and still empty.
    let mut created: Vec<&Recorded> = all
        .iter()
        .filter(|r| r.created && !failed_paths.contains(r.path.as_str()))
        .collect();
    created.sort_by_key(|r| std::cmp::Reverse(r.path.len()));
    for r in created {
        let same = open_no_follow(&r.path, FILE_READ_ATTRIBUTES)
            .is_ok_and(|o| o.is_dir && skip_reason(o.attrs, true).is_none() && r.file_id.is_none_or(|id| id == o.id));
        if same && std::fs::remove_dir(&r.path).is_ok() {
            out.placeholders_removed += 1;
        }
    }
    if out.failed.is_empty() {
        clear_marker(store.key)?;
        if store.key == persist::MARKER_KEY {
            persist::marker_clear_ever_if_unused(store.conn)?;
        }
    } else {
        write_marker(
            store.key,
            &Marker {
                notes: Vec::new(),
                header: marker.header.map(|h| Header { complete: false, ..h }),
                extra: marker.extra,
                env_extra: marker.env_extra,
                exclusions: marker.exclusions,
                ungranted_only: marker.ungranted_only,
                paths: all
                    .iter()
                    .filter(|r| failed_paths.contains(r.path.as_str()))
                    .cloned()
                    .collect(),
            },
        )?;
    }
    Ok(out)
}

/// `fence-helper uninstall`: forget the calling user's profile grant.
/// Does nothing, and creates no state, when none is recorded.
pub fn forget_everything() -> Result<Option<Forgotten>> {
    forget_everything_as(None)
}

/// [`forget_everything`] with the identities read earlier: after the
/// install record is gone, `ids` taken before it went still name the
/// group the grant was written for.
pub fn forget_everything_as(ids: Option<&Ids>) -> Result<Option<Forgotten>> {
    let marker_empty = read_marker_at(persist::MARKER_KEY).is_ok_and(|m| m.is_empty());
    let db_empty = match crate::state_db::open_db_ro() {
        Ok(Some(conn)) => db_has_rows(&conn).is_ok_and(|rows| !rows),
        Ok(None) => true,
        Err(_) => false,
    };
    if marker_empty && db_empty {
        return Ok(None);
    }
    let loaded;
    let ids = match ids {
        Some(ids) => ids,
        None => {
            loaded = Ids::load()?;
            &loaded
        }
    };
    // Under the one lock for the whole run, like every change to the
    // profile's entries.
    crate::state_db::with_profile_lock(|db| {
        crate::state_db::mark_profile_operation()?;
        forget(&Store::of(db.conn()), ids)
    })
    .map(Some)
}

// ─── Status ─────────────────────────────────────────────────────────

/// Whether the file at `path` carries its own recorded entry and is no
/// longer the object that entry was written on.
fn replaced_since(paths: &[Recorded], path: &str) -> bool {
    let p = path_id::strip_extended_prefix(path).to_lowercase();
    paths.iter().any(|r| {
        r.kind == Kind::File
            && path_id::strip_extended_prefix(&r.path).to_lowercase() == p
            && r.file_id
                .is_some_and(|id| path_id::capture_file_id(path).is_ok_and(|now| now != id))
    })
}

/// Whether `p` lies strictly inside the directory `dir` (both lower
/// case, no `\\?\` prefix).
fn is_beneath(dir: &str, p: &str) -> bool {
    p.len() > dir.len() && p.starts_with(dir) && p.as_bytes()[dir.len()] == b'\\'
}

/// Whether `path` is a directory a sandboxed command may enter and list:
/// recorded as granted whole or as a container, or beneath a granted one.
fn traversable(paths: &[Recorded], path: &str) -> bool {
    let p = path_id::strip_extended_prefix(path).to_lowercase();
    paths.iter().any(|r| {
        let at = path_id::strip_extended_prefix(&r.path).to_lowercase();
        match r.kind {
            Kind::Granted | Kind::Container if at == p => true,
            Kind::Granted => is_beneath(&at, &p),
            _ => false,
        }
    })
}

/// Whether `path` (as a tool would be given it, without the `\\?\`
/// prefix) is a file the grant makes readable: recorded as a granted
/// file, or beneath a recorded granted directory.
fn covered(paths: &[Recorded], path: &str) -> bool {
    let p = path_id::strip_extended_prefix(path).to_lowercase();
    paths.iter().any(|r| {
        let at = path_id::strip_extended_prefix(&r.path).to_lowercase();
        match r.kind {
            Kind::File => at == p,
            Kind::Granted => is_beneath(&at, &p),
            _ => false,
        }
    })
}

/// `profile status`: what is recorded, and whether it is a complete
/// grant of the current list for the profile that is there now.
pub fn status_json(key: &str, layout: Result<Layout>, with_paths: bool) -> serde_json::Value {
    let m = match read_marker_at(key) {
        Ok(m) => m,
        Err(e) => return json!({ "state": "unknown", "reason": format!("{e:#}") }),
    };
    if m.is_empty() {
        return json!({
            "state": "none",
            "record_version": RECORD_VERSION,
            "list_version": plan::LIST_VERSION,
        });
    }
    let count = |k: Kind| m.paths.iter().filter(|r| r.kind == k).count();
    let mut out = json!({
        "record_version": RECORD_VERSION,
        "list_version": plan::LIST_VERSION,
        "recorded_list_version": m.header.as_ref().map(|h| h.list_version),
        "profile": m.header.as_ref().map(|h| h.profile.as_str()),
        "extra_exclusions": m.extra,
        "env_exclusions": m.env_extra,
        "exclusions": m.exclusions,
        "ungranted_only": m.ungranted_only,
        "containers": count(Kind::Container),
        "granted": count(Kind::Granted),
        "files": count(Kind::File),
        "denied": count(Kind::Denied),
    });
    // An exclusion without a deny is looked at again now, not reported
    // as the run classed it: access lists change. A failed look is "unverifiable".
    let ids = Ids::load().ok();
    let mut by_class: std::collections::BTreeMap<&str, Vec<Item>> =
        NoteClass::ALL.into_iter().map(|c| (c.as_str(), Vec::new())).collect();
    for n in &m.notes {
        let (class, item) = match (n.class, &ids) {
            (NoteClass::NotGranted, _) => (
                n.class,
                Item {
                    path: n.path.clone(),
                    reason: n.reason.clone(),
                    ..Default::default()
                },
            ),
            // Looked at again: no longer reported once the entry is gone.
            (NoteClass::StaleAllow, Some(ids)) => match own_access(&n.path, ids) {
                Ok(Some(Access { ours: None, .. })) | Ok(None) => continue,
                found => (
                    n.class,
                    Item {
                        path: n.path.clone(),
                        reason: n.reason.clone(),
                        protected_exclusion: Some(false),
                        entry: match found {
                            Ok(Some(Access { ours, .. })) => ours,
                            _ => None,
                        },
                        ..Default::default()
                    },
                ),
            },
            (NoteClass::StaleAllow, None) => (
                n.class,
                Item {
                    path: n.path.clone(),
                    reason: n.reason.clone(),
                    protected_exclusion: Some(false),
                    ..Default::default()
                },
            ),
            // A directory above an exclusion, still granted whole:
            // as the run found it, until the next run.
            (NoteClass::OurAllowAbove, _) => (
                NoteClass::Unprotected,
                Item {
                    path: n.path.clone(),
                    reason: n.reason.clone(),
                    protected_exclusion: Some(true),
                    entry: Some("explicit"),
                    ..Default::default()
                },
            ),
            // Still so unless the entry is seen to be gone; an access list
            // that cannot be read does not take back what the run found.
            (NoteClass::OurAllow, Some(ids)) => {
                // Looked at once.
                let probe = exclusion_as_it_is(&n.path, ids, "could not be written when the grant ran");
                let stands = probe.entry.is_some() || probe.class == NoteClass::Unverifiable;
                if stands {
                    (
                        NoteClass::Unprotected,
                        Item {
                            path: n.path.clone(),
                            reason: n.reason.clone(),
                            protected_exclusion: Some(true),
                            entry: Some(probe.entry.unwrap_or("explicit")),
                            ..Default::default()
                        },
                    )
                } else {
                    (probe.class, probe.item(&n.path))
                }
            }
            (_, Some(ids)) => {
                let probe = exclusion_as_it_is(&n.path, ids, "could not be written when the grant ran");
                (probe.class, probe.item(&n.path))
            }
            // Whose reading is to be judged is not known: no claim is
            // made either way.
            (_, None) => {
                let probe = Probe::unverifiable(
                    "account_unknown",
                    None,
                    "it could not be examined: the sandbox account is not known".to_owned(),
                );
                (probe.class, probe.item(&n.path))
            }
        };
        by_class.entry(class.as_str()).or_default().push(item);
    }
    for (class, items) in by_class {
        out[class] = json!(items);
    }
    if with_paths {
        out["paths"] = m
            .paths
            .iter()
            .map(|r| json!({ "path": r.path, "kind": r.kind.as_str() }))
            .collect();
    }
    let (state, reason): (&str, Option<String>) = match (&m.header, &layout) {
        (None, _) => (
            "incomplete",
            Some("paths are recorded without a grant: a removal did not finish".to_owned()),
        ),
        (Some(h), _) if !h.complete => (
            "incomplete",
            Some("the last run did not finish, or could not remove an entry of an earlier run".to_owned()),
        ),
        (Some(h), _) if h.list_version != plan::LIST_VERSION => (
            "outdated",
            Some(format!(
                "made with list version {}; this build has {}",
                h.list_version,
                plan::LIST_VERSION
            )),
        ),
        (Some(_), Err(e)) => ("unknown", Some(format!("{e:#}"))),
        (Some(h), Ok(l)) if h.profile.to_lowercase() != l.profile.to_lowercase() => (
            "outdated",
            Some(format!("made for '{}'; the profile is '{}'", h.profile, l.profile)),
        ),
        (Some(h), Ok(l)) => match open_no_follow(&l.profile, FILE_READ_ATTRIBUTES) {
            Ok(o) if o.id == h.file_id => ("current", None),
            Ok(_) => (
                "outdated",
                Some("the profile directory is not the one the grant was made on".to_owned()),
            ),
            Err(e) => ("unknown", Some(format!("{e:#}"))),
        },
    };
    out["state"] = json!(state);
    if let Some(r) = reason {
        out["reason"] = json!(r);
    }
    if let (true, Ok(l)) = (state == "current", &layout) {
        let shown = |p: &str| path_id::strip_extended_prefix(p).to_owned();
        // A file granted by its own entry and replaced since (a save by
        // write-and-rename) has no entry: not readable, and said.
        let replaced = std::cell::RefCell::new(Vec::<String>::new());
        let readable = |p: &str| {
            if !(std::fs::metadata(p).is_ok_and(|md| md.is_file()) && covered(&m.paths, p)) {
                return false;
            }
            let gone = replaced_since(&m.paths, p);
            if gone && !replaced.borrow().iter().any(|r| r == p) {
                replaced.borrow_mut().push(p.to_owned());
            }
            !gone
        };
        // A directory: a real one (not a link to somewhere else),
        // which the grant made traversable.
        let dir_readable =
            |p: &str| std::fs::symlink_metadata(p).is_ok_and(|md| md.is_dir()) && traversable(&m.paths, p);
        let mut withheld = serde_json::Map::new();
        let env: serde_json::Map<String, serde_json::Value> = plan::env_pointers(
            &shown(&l.profile),
            l.roaming.as_deref().map(shown).as_deref(),
            &readable,
            &dir_readable,
            &|var| l.own(var),
        )
        .into_iter()
        // `CARGO_HOME` only where its credential files are on record as
        // protected: a grant made before it pointed there has not excluded them.
        .filter(|(var, dir)| {
            let ok = *var != "CARGO_HOME" || plan::cargo_home_protected(&m.exclusions, dir);
            if !ok {
                withheld.insert(
                    (*var).to_owned(),
                    json!(format!(
                        "'{dir}' was not the cargo home when the grant was made: its \
                         credential files are not excluded. Run `fence-helper profile grant` again."
                    )),
                );
            }
            ok
        })
        .map(|(k, v)| (k.to_owned(), json!(v)))
        .collect();
        out["env"] = serde_json::Value::Object(env);
        if !withheld.is_empty() {
            out["env_withheld"] = serde_json::Value::Object(withheld);
        }
        out["replaced_files"] = json!(*replaced.borrow());
    }
    out
}

#[cfg(test)]
mod tests;
