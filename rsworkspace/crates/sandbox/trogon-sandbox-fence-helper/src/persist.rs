//! Write grants kept between sessions, for the write-restricted token.
//! Each write root gets its own capability SID,
//! `S-1-0-1953719907-<a>-<b>-<c>-<d>` (128 random bits; no account can
//! hold it), and its ACEs ([`crate::acl::persistent_aces`]) allow
//! writing only to a token with that SID among its restricting SIDs.
//! A marker under `HKCU\Software\trogon-sandbox\PersistentGrants` lists
//! every root that may carry a grant. While it or the session database
//! lists a root, nothing may run the sandbox account under the ordinary
//! token ([`unrestricted_block_reason`]). A root whose volume is away
//! becomes a [`PendingStrip`] there.
use crate::context::Context;
use crate::error::{FenceHelperError, Result};
use rusqlite::{Connection, OptionalExtension, params};
use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};

use crate::acl::{self, AceView, DaclTarget, PersistentTrustees, PersistentWant, SbAce};
use crate::path_id::{self, FileId, Located};
use crate::reg;
use crate::state_db::Locked;

// ─── Capability SIDs ────────────────────────────────────────────────

/// First sub-authority of every capability SID: ASCII `"tsfc"`.
const CAPABILITY_TAG: u32 = 0x7473_6663;

/// What every capability SID starts with - see the module doc.
pub const CAPABILITY_SID_PREFIX: &str = "S-1-0-1953719907-";

/// The capability SID made of `random` (four little-endian `u32`s).
pub fn capability_sid_from(random: [u8; 16]) -> String {
    let mut s = format!("S-1-0-{CAPABILITY_TAG}");
    for c in random.as_chunks::<4>().0 {
        s.push_str(&format!("-{}", u32::from_le_bytes(*c)));
    }
    s
}

/// A fresh capability SID from the system CSPRNG. An RNG failure is
/// an error, never a predictable SID.
pub fn new_capability_sid() -> Result<String> {
    let mut r = [0u8; 16];
    unsafe { BCryptGenRandom(None, &mut r, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .context("BCryptGenRandom (capability SID)")?;
    Ok(capability_sid_from(r))
}

/// Whether `sid` is a capability SID in its canonical spelling: the
/// prefix and exactly four decimal `u32` sub-authorities.
pub fn is_capability_sid(sid: &str) -> bool {
    let Some(rest) = sid.strip_prefix(CAPABILITY_SID_PREFIX) else {
        return false;
    };
    let mut n = 0;
    rest.split('-').all(|p| {
        n += 1;
        p.parse::<u32>().is_ok_and(|v| v.to_string() == p)
    }) && n == 4
}

// ─── Marker ─────────────────────────────────────────────────────────

pub(crate) const MARKER_KEY: &str = r"Software\trogon-sandbox\PersistentGrants";
const MARKER_ROOTS: &str = "Roots";
const MARKER_EVER: &str = "EverEnabled";
const MARKER_PENDING: &str = "Pending";
const MARKER_GROUP: &str = "GroupSid";
const MARKER_PREVIOUS_GROUPS: &str = "PreviousGroupSids";

/// Record the sandbox group's SID beside the grants that name it, so they
/// can be removed after the install record is gone. A different SID
/// recorded earlier is kept as a previous group, still ours to remove.
pub(crate) fn marker_set_group_sid(group_sid: &str) -> Result<()> {
    let key = reg::Key::create_hkcu(MARKER_KEY)?;
    if let Some(old) = key.get_sz(MARKER_GROUP)?.filter(|s| !s.is_empty())
        && old != group_sid
    {
        let mut previous = key.get_multi_sz(MARKER_PREVIOUS_GROUPS)?.unwrap_or_default();
        if !previous.contains(&old) {
            previous.push(old);
            key.set_multi_sz(MARKER_PREVIOUS_GROUPS, &previous)?;
        }
    }
    key.set_sz(MARKER_GROUP, group_sid)
}

/// Set the marker's `EverEnabled`: from now on "nothing is recorded"
/// is believed only from a state database that can be read.
pub(crate) fn marker_set_ever() -> Result<()> {
    reg::Key::create_hkcu(MARKER_KEY)?.set_dword(MARKER_EVER, 1)
}

/// Clear `EverEnabled` once nothing is recorded any more: no root in
/// the marker or the database, and no profile read grant.
pub(crate) fn marker_clear_ever_if_unused(conn: &Connection) -> Result<()> {
    let m = read_marker()?;
    if m.roots.is_empty()
        && m.pending.is_empty()
        && all_root_paths(conn)?.is_empty()
        && !crate::profile::recorded(conn)?
        && let Some(key) = reg::Key::open_hkcu(MARKER_KEY, true)?
    {
        reg::delete_value(&key, MARKER_EVER)?;
        // With nothing recorded, no entry of an earlier group is left
        // to recognise either.
        if key.get_multi_sz(MARKER_PREVIOUS_GROUPS)?.is_some() {
            reg::delete_value(&key, MARKER_PREVIOUS_GROUPS)?;
        }
    }
    Ok(())
}

/// One root in the marker: where it was granted and which object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerRoot {
    pub file_id: FileId,
    pub path: String,
}

impl MarkerRoot {
    /// `<48 hex digits of the file id>|<canonical path>`.
    pub fn encode(&self) -> String {
        let hex: String = self.file_id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        format!("{hex}|{}", self.path)
    }

    pub fn decode(s: &str) -> Result<Self> {
        let (hex, path) = s
            .split_once('|')
            .filter(|(h, p)| h.len() == 48 && h.is_ascii() && !p.is_empty())
            .ok_or_else(|| FenceHelperError::msg(format!("malformed persistent-grant marker entry '{s}'")))?;
        let bytes = (0..24)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .with_context(|| format!("malformed persistent-grant marker entry '{s}'"))?;
        Ok(Self {
            file_id: FileId::from_bytes(&bytes)?,
            path: path.to_owned(),
        })
    }
}

/// A kept grant whose removal is waiting for its volume: the object,
/// the sandbox group its ACEs name, and the path it was last known
/// at (for messages; the file id is what finds it again).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingStrip {
    pub file_id: FileId,
    pub group_sid: String,
    pub path: String,
}

impl PendingStrip {
    /// `<48 hex digits of the file id>|<group SID>|<path>`.
    pub fn encode(&self) -> String {
        let hex: String = self.file_id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        format!("{hex}|{}|{}", self.group_sid, self.path)
    }

    pub fn decode(s: &str) -> Result<Self> {
        let bad = || FenceHelperError::msg(format!("malformed pending-strip marker entry '{s}'"));
        let mut it = s.splitn(3, '|');
        let (hex, group, path) = (
            it.next().ok_or_else(bad)?,
            it.next().ok_or_else(bad)?,
            it.next().ok_or_else(bad)?,
        );
        let root = MarkerRoot::decode(&format!("{hex}|{path}")).map_err(|_| bad())?;
        Ok(Self {
            file_id: root.file_id,
            group_sid: group.to_owned(),
            path: root.path,
        })
    }
}

/// The marker as read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marker {
    /// A persistent grant was made at some point and `acl forget
    /// --all` has not completed since.
    pub ever_enabled: bool,
    pub roots: Vec<MarkerRoot>,
    /// Kept grants waiting for their volume to be removed.
    pub pending: Vec<PendingStrip>,
    /// The sandbox group the recorded grants name.
    pub group_sid: Option<String>,
    /// Groups of earlier installs that recorded grants may still name.
    pub previous_group_sids: Vec<String>,
}

impl Marker {
    /// No root, no pending strip, and no grant was ever made.
    fn nothing_recorded(&self) -> bool {
        !self.ever_enabled && self.roots.is_empty() && self.pending.is_empty()
    }
}

/// Read the marker. An absent key is the empty marker; anything that
/// cannot be read or parsed is an error, which callers treat as
/// "grants may exist".
pub fn read_marker() -> Result<Marker> {
    let Some(key) = reg::Key::open_hkcu(MARKER_KEY, false)? else {
        return Ok(Marker::default());
    };
    let roots = key
        .get_multi_sz(MARKER_ROOTS)?
        .unwrap_or_default()
        .iter()
        .map(|s| MarkerRoot::decode(s))
        .collect::<Result<_>>()?;
    let pending = key
        .get_multi_sz(MARKER_PENDING)?
        .unwrap_or_default()
        .iter()
        .map(|s| PendingStrip::decode(s))
        .collect::<Result<_>>()?;
    Ok(Marker {
        ever_enabled: key.get_dword(MARKER_EVER)?.unwrap_or(0) != 0,
        roots,
        pending,
        group_sid: key.get_sz(MARKER_GROUP)?.filter(|s| !s.is_empty()),
        previous_group_sids: key.get_multi_sz(MARKER_PREVIOUS_GROUPS)?.unwrap_or_default(),
    })
}

fn write_marker_roots(roots: &[MarkerRoot]) -> Result<()> {
    let key = reg::Key::create_hkcu(MARKER_KEY)?;
    let encoded: Vec<String> = roots.iter().map(MarkerRoot::encode).collect();
    key.set_multi_sz(MARKER_ROOTS, &encoded)
}

/// List `path` (object `file_id`) in the marker, replacing an entry
/// for the same path. `EverEnabled` is set first.
fn marker_add(path: &str, file_id: FileId) -> Result<()> {
    let mut m = read_marker()?;
    marker_set_ever()?;
    let entry = MarkerRoot {
        file_id,
        path: path.to_owned(),
    };
    if m.roots.contains(&entry) {
        return Ok(());
    }
    m.roots.retain(|r| r.path != path);
    m.roots.push(entry);
    write_marker_roots(&m.roots)
}

fn write_pending(pending: &[PendingStrip]) -> Result<()> {
    let key = reg::Key::create_hkcu(MARKER_KEY)?;
    let encoded: Vec<String> = pending.iter().map(PendingStrip::encode).collect();
    key.set_multi_sz(MARKER_PENDING, &encoded)
}

/// Add pending strips to the marker (an object already listed is not
/// listed twice).
pub(crate) fn marker_add_pending(new: &[PendingStrip]) -> Result<()> {
    if new.is_empty() {
        return Ok(());
    }
    let mut pending = read_marker()?.pending;
    for p in new {
        if !pending.iter().any(|q| q.file_id == p.file_id) {
            pending.push(p.clone());
        }
    }
    write_pending(&pending)
}

fn marker_remove(path: &str) -> Result<()> {
    let mut m = read_marker()?;
    if m.roots.iter().any(|r| r.path == path) {
        m.roots.retain(|r| r.path != path);
        write_marker_roots(&m.roots)?;
    }
    Ok(())
}

// ─── Running without the write-restricted token ─────────────────────

/// Root paths the session database records, or `None` if it is
/// missing, unreadable, or has no such table.
fn db_roots_ro() -> Option<Vec<String>> {
    let conn = crate::state_db::open_db_ro().ok()??;
    all_root_paths(&conn).ok()
}

/// Why a command must not run the sandbox account WITHOUT the
/// write-restricted token now; `None` when it may. "Nothing recorded"
/// is accepted only from a store that is there to be read.
pub fn unrestricted_block_reason() -> Option<String> {
    write_grant_block_reason().or_else(crate::profile::block_reason)
}

/// [`unrestricted_block_reason`] for the write grants alone. A
/// profile read grant refuses the ordinary token under the same rule
/// ([`crate::profile::block_reason`]) and has its own removal.
pub fn write_grant_block_reason() -> Option<String> {
    block_reason_from(read_marker(), db_roots_ro, path_id::volume_mounted)
}

/// [`unrestricted_block_reason`] on what was read: `marker` as
/// [`read_marker`] returned it, `db_roots` as [`db_roots_ro`] does.
/// Every "could not read" refuses.
fn block_reason_from(
    marker: Result<Marker>,
    db_roots: impl FnOnce() -> Option<Vec<String>>,
    mounted: impl Fn(u64) -> bool,
) -> Option<String> {
    let m = match marker {
        Ok(m) => m,
        Err(e) => {
            return Some(format!("the persistent-grant marker cannot be read ({e:#})"));
        }
    };
    if let Some(r) = m.roots.first() {
        return Some(format!(
            "{} persistent write grant(s) exist (e.g. '{}')",
            m.roots.len(),
            r.path
        ));
    }
    if let Some(p) = m.pending.iter().find(|p| mounted(p.file_id.volume_serial)) {
        return Some(format!(
            "the persistent write grant on '{}' is still on disk: its volume was absent \
             when it was forgotten and is mounted again",
            p.path
        ));
    }
    if !m.ever_enabled {
        return None;
    }
    match db_roots() {
        Some(v) if v.is_empty() => None,
        Some(v) => Some(format!(
            "the state database records {} persistent write grant(s) (e.g. '{}')",
            v.len(),
            v[0]
        )),
        None => Some(
            "persistent write grants were made and the state database is missing or \
             unreadable, so where they are cannot be established"
                .to_owned(),
        ),
    }
}

/// `{state, roots, reason?}` for `fence-helper user status`: `none` when
/// the ordinary token may be used, `present` when the marker lists
/// roots, `unknown` otherwise.
pub fn status_json() -> serde_json::Value {
    let m = read_marker().unwrap_or_default();
    let roots: Vec<String> = m.roots.into_iter().map(|r| r.path).collect();
    // Grants waiting for an absent volume: listed, and no reason to
    // refuse the ordinary token until the volume is back.
    let pending: Vec<String> = m.pending.into_iter().map(|p| p.path).collect();
    match write_grant_block_reason() {
        None => serde_json::json!({ "state": "none", "roots": roots, "pending": pending }),
        Some(reason) => serde_json::json!({
            "state": if roots.is_empty() { "unknown" } else { "present" },
            "roots": roots,
            "pending": pending,
            "reason": reason,
        }),
    }
}

// ─── Identities ─────────────────────────────────────────────────────

mod ids;
pub use ids::Ids;

// ─── Rows ───────────────────────────────────────────────────────────

/// Every root that may carry a kept write grant: the marker's and the
/// database's.
pub(crate) fn kept_roots(conn: &Connection) -> Result<Vec<String>> {
    let mut roots: Vec<String> = read_marker()?.roots.into_iter().map(|r| r.path).collect();
    for r in all_root_paths(conn)? {
        if !roots.contains(&r) {
            roots.push(r);
        }
    }
    Ok(roots)
}

fn all_root_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut s = conn.prepare("SELECT canonical_path FROM persistent_roots")?;
    let rows = s.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn root_row(conn: &Connection, canon: &str) -> Result<Option<(FileId, String)>> {
    let row: Option<(Vec<u8>, String)> = conn
        .query_row(
            "SELECT file_id, capability_sid FROM persistent_roots \
             WHERE canonical_path = ?1",
            params![canon],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .context("SELECT persistent_roots")?;
    row.map(|(id, sid)| Ok((FileId::from_bytes(&id)?, sid))).transpose()
}

/// The longest of `roots` that is `canon` or a directory above it.
pub fn covering_root<'a>(roots: &'a [String], canon: &str) -> Option<&'a str> {
    roots
        .iter()
        .map(String::as_str)
        .filter(|r| {
            canon == *r
                || (canon.starts_with(r) && (r.ends_with('\\') || canon.as_bytes().get(r.len()) == Some(&b'\\')))
        })
        .max_by_key(|r| r.len())
}

/// The ACEs `canon` (now the object `cur`) is to carry, from the
/// rows. A row recorded for a different object at this path is not
/// applied to the one that is there now.
fn wanted(conn: &Connection, ids: &Ids, canon: &str, cur: FileId) -> Result<Vec<AceView>> {
    let cap = root_row(conn, canon)?.filter(|(id, _)| *id == cur).map(|(_, sid)| sid);
    let mut s = conn.prepare_cached("SELECT kind, mask, file_id FROM persistent_denies WHERE canonical_path = ?1")?;
    let rows = s.query_map(params![canon], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?))
    })?;
    let (mut deny, mut deny_fdc, mut deny_delete) = (None, false, false);
    for row in rows {
        let (kind, mask, id) = row?;
        if FileId::from_bytes(&id)? != cur {
            continue;
        }
        match SbAce::parse(&kind, &mask)? {
            SbAce::Deny(d) => deny = Some(d),
            SbAce::DenyFdc => deny_fdc = true,
            SbAce::DenyDelete => deny_delete = true,
            SbAce::Grant(_) => {
                return Err(FenceHelperError::msg(format!(
                    "persistent_denies holds a grant row for '{canon}'"
                )));
            }
        }
    }
    // What the profile read grant recorded for this object: an
    // excluded path is a read deny, any other kind one allow.
    let mut profile = None;
    match crate::profile::recorded_kind(conn, canon, cur)? {
        Some(crate::profile_plan::Kind::Denied) => deny = Some(acl::DenyMask::ReadDeny),
        Some(crate::profile_plan::Kind::Container) => profile = Some(acl::ProfileAce::Container),
        Some(crate::profile_plan::Kind::Granted) => profile = Some(acl::ProfileAce::Granted),
        Some(crate::profile_plan::Kind::File) => profile = Some(acl::ProfileAce::File),
        Some(crate::profile_plan::Kind::Skipped | crate::profile_plan::Kind::Ungranted) | None => {}
    }
    if cap.is_none() && deny.is_none() && !deny_fdc && !deny_delete && profile.is_none() {
        return Ok(Vec::new());
    }
    let group_sid = ids.group_sid.as_deref().ok_or_else(|| {
        FenceHelperError::msg(
            "no install record: the sandbox group SID is unknown - run `fence-helper install`".to_string(),
        )
    })?;
    Ok(acl::persistent_aces(&PersistentWant {
        group_sid,
        real_user_sid: &ids.real_user_sid,
        capability_sid: cap.as_deref(),
        deny,
        deny_fdc,
        deny_delete,
        profile,
    }))
}

/// The wanted ACE list as the text a write record stores.
fn spec_of(want: &[AceView]) -> String {
    want.iter().map(AceView::sddl).collect()
}

/// The DENY entries of a write record's text ([`spec_of`]): trustee
/// and mask of each `(D;…)`. An entry that does not read as one is
/// left out.
fn denies_of_spec(spec: &str) -> Vec<AceView> {
    spec.split(')')
        .filter_map(|ace| {
            let mut f = ace.trim_start_matches('(').split(';');
            let (ty, _flags, mask) = (f.next()?, f.next()?, f.next()?);
            let sid = f.nth(2)?;
            let mask = u32::from_str_radix(mask.strip_prefix("0x")?, 16).ok()?;
            (ty == "D").then(|| AceView {
                deny: true,
                inherited: false,
                flags: 0,
                mask,
                sid: sid.to_owned(),
            })
        })
        .collect()
}

/// The denies the record lists for a path: those wanted now and those
/// of its last write (`last_spec`; they may be on the object still).
fn recorded_denies(want: &[AceView], last_spec: Option<&str>) -> Option<Vec<AceView>> {
    let last = last_spec?;
    let mut v: Vec<AceView> = want.iter().filter(|a| a.deny).cloned().collect();
    v.extend(denies_of_spec(last));
    Some(v)
}

// ─── Converge ───────────────────────────────────────────────────────

/// Bring `canon`'s persistent ACEs to what the rows say; returns
/// whether it wrote. The write is skipped only when the ACEs on disk
/// match AND the write record says that exact write completed.
fn converge(conn: &Connection, ids: &Ids, canon: &str) -> Result<bool> {
    converge_on(conn, ids, canon, None)
}

/// An object that is already open, for [`converge_on`]: the handle
/// (with `READ_CONTROL | WRITE_DAC`), and the identity read from it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OpenObject {
    pub handle: windows::Win32::Foundation::HANDLE,
    pub id: FileId,
    pub is_dir: bool,
}

/// [`converge`], reading and writing through `on` when given instead
/// of opening `canon` by name.
pub(crate) fn converge_on(conn: &Connection, ids: &Ids, canon: &str, on: Option<&OpenObject>) -> Result<bool> {
    let (cur, is_dir) = match on {
        Some(o) => (o.id, o.is_dir),
        None => {
            let (cur, _links, is_dir) = path_id::capture_id_and_links(canon)?;
            (cur, is_dir)
        }
    };
    let target = match on {
        Some(o) => DaclTarget::Handle(o.handle, canon),
        None => DaclTarget::Path(canon),
    };
    let want = wanted(conn, ids, canon, cur)?;
    let aces = acl::read_aces_at(target).with_context(|| format!("read the DACL of '{canon}'"))?;
    let is_root = want.iter().any(|a| is_capability_sid(&a.sid));
    if is_root {
        // An ALLOW for the sandbox user SID would pass both access
        // checks of every session's token.
        if let Some(sb) = &ids.sandbox_user_sid
            && acl::allows_more_than_read(&aces, sb)
        {
            return Err(FenceHelperError::msg(format!(
                "'{canon}' grants write access to the sandbox user SID (a session without \
                 the write-restricted token holds it); refusing to keep a persistent grant \
                 beside it"
            )));
        }
        for ace in acl::owner_write_paths(&aces) {
            eprintln!(
                "fence-helper: WARNING: '{canon}' carries {ace}: files the sandbox creates under \
                 it stay writable to sessions of other projects"
            );
        }
    }
    let spec = spec_of(&want);
    let record: Option<(Vec<u8>, String, i64)> = conn
        .query_row(
            "SELECT file_id, spec, done FROM persistent_writes WHERE canonical_path = ?1",
            params![canon],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .context("SELECT persistent_writes")?;
    // A group deny the record does not list is not ours: it stays.
    let listed = recorded_denies(&want, record.as_ref().map(|r| r.1.as_str()));
    let t = ids.trustees_for(listed.as_deref());
    // The same entries, with no explicit deny behind one of our allows.
    let same = acl::persistent_on_disk(&aces, &t, is_dir) == acl::comparable(&want, is_dir)
        && acl::persistent_layout_ok(&aces, &t);
    // Settled: no write is outstanding for this path, and (with ACEs
    // wanted) a write of exactly this list completed on this object.
    let settled = is_settled(record.as_ref(), want.is_empty(), &spec, &cur.as_bytes());
    if same && settled {
        if want.is_empty() {
            conn.execute(
                "DELETE FROM persistent_writes WHERE canonical_path = ?1",
                params![canon],
            )?;
        }
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO persistent_writes (canonical_path, file_id, spec, done) \
         VALUES (?1, ?2, ?3, 0) \
         ON CONFLICT(canonical_path) DO UPDATE SET \
           file_id = excluded.file_id, spec = excluded.spec, done = 0",
        params![canon, cur.as_bytes().as_slice(), spec],
    )
    .context("record persistent write intent")?;
    // The walk is spared only where nothing handed down changes, and
    // never where the record cannot vouch that an earlier write finished.
    let unfinished = matches!(&record, Some((_, _, 0)));
    let written = acl::write_persistent_aces_at(target, &t, &want, unfinished || same)?;
    if want.is_empty() {
        for left in acl::foreign_group_denies(&aces, &t) {
            eprintln!(
                "fence-helper: '{canon}' carries a deny for the sandbox group that this product \
                 did not write, {}: left in place",
                left.sddl()
            );
        }
    }
    if want.is_empty() {
        conn.execute(
            "DELETE FROM persistent_writes WHERE canonical_path = ?1",
            params![canon],
        )
    } else {
        conn.execute(
            "UPDATE persistent_writes SET done = 1 WHERE canonical_path = ?1",
            params![canon],
        )
    }
    .context("record persistent write completion")?;
    if std::env::var_os("TROGON_SANDBOX_FENCE_DEBUG").is_some() {
        eprintln!(
            "fence-helper: persistent DACL write '{canon}' ({} ACE(s), {written:?})",
            want.len()
        );
    }
    Ok(true)
}

/// Whether the write record vouches for a path: no write is outstanding
/// and, with ACEs wanted, a write of exactly `spec` completed on `cur`.
fn is_settled(record: Option<&(Vec<u8>, String, i64)>, want_empty: bool, spec: &str, cur: &[u8]) -> bool {
    match record {
        None => want_empty,
        Some((id, s, done)) => *done == 1 && (want_empty || (s == spec && id.as_slice() == cur)),
    }
}

/// Put on record, outside any transaction, that the strip of `root` and
/// of the deny targets kept with it has begun. A strip that is killed
/// mid-walk then reads as unfinished and is walked again.
fn mark_strip_begun(conn: &Connection, root: &str) -> Result<()> {
    conn.execute(
        "UPDATE persistent_writes SET done = 0 WHERE canonical_path = ?1 \
         OR canonical_path IN (SELECT canonical_path FROM persistent_denies WHERE root = ?1)",
        params![root],
    )
    .context("record the strip as begun")?;
    Ok(())
}

/// After a [`converge_on`] that failed: whether the object itself
/// carries exactly the wanted ACEs all the same.
pub(crate) fn settled_on_object(conn: &Connection, ids: &Ids, canon: &str, on: &OpenObject) -> Result<bool> {
    let want = wanted(conn, ids, canon, on.id)?;
    let aces = acl::read_aces_at(DaclTarget::Handle(on.handle, canon))?;
    let last: Option<String> = conn
        .query_row(
            "SELECT spec FROM persistent_writes WHERE canonical_path = ?1",
            params![canon],
            |r| r.get(0),
        )
        .optional()?;
    let listed = recorded_denies(&want, last.as_deref());
    let t = ids.trustees_for(listed.as_deref());
    if acl::persistent_on_disk(&aces, &t, on.is_dir) != acl::comparable(&want, on.is_dir)
        || !acl::persistent_layout_ok(&aces, &t)
    {
        return Ok(false);
    }
    if want.is_empty() {
        conn.execute(
            "DELETE FROM persistent_writes WHERE canonical_path = ?1",
            params![canon],
        )?;
    } else {
        conn.execute(
            "UPDATE persistent_writes SET done = 1 WHERE canonical_path = ?1",
            params![canon],
        )?;
    }
    Ok(true)
}

/// Crash recovery: finish every DACL write whose record was left at
/// `done = 0`. A record whose path is gone or names another object is
/// dropped; a failure is logged and the record kept.
pub(crate) fn replay_pending(conn: &Connection) {
    let read = || -> Result<Vec<(String, Vec<u8>)>> {
        let mut s = conn.prepare("SELECT canonical_path, file_id FROM persistent_writes WHERE done = 0")?;
        let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    let pending = match read() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("fence-helper: WARNING: reading pending persistent writes: {e:#}");
            return;
        }
    };
    if pending.is_empty() {
        return;
    }
    let ids = match Ids::load() {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("fence-helper: WARNING: pending persistent writes not replayed: {e:#}");
            return;
        }
    };
    for (canon, id) in pending {
        let stale = match path_id::capture_file_id(&canon) {
            Ok(cur) => cur.as_bytes().as_slice() != id.as_slice(),
            Err(e) => path_id::is_not_found(&e),
        };
        let r = if stale {
            conn.execute(
                "DELETE FROM persistent_writes WHERE canonical_path = ?1",
                params![canon],
            )
            .map(|_| ())
            .map_err(FenceHelperError::from)
        } else {
            converge(conn, &ids, &canon).map(|_| ())
        };
        if let Err(e) = r {
            eprintln!("fence-helper: WARNING: replay persistent write '{canon}': {e:#}");
        }
    }
}

// ─── Grant ──────────────────────────────────────────────────────────

/// Outcome of [`apply`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    pub roots: usize,
    /// DACL writes that walked a tree: the system brought the
    /// inherited entries of everything below the path up to date.
    pub writes: usize,
    /// DACL writes on an object alone ([`acl::Written::Local`]).
    pub local_writes: usize,
    /// Paths found as wanted and left alone.
    pub unchanged: usize,
}

/// Make sure `canon` has a capability SID recorded for the object
/// that is there now, and that the marker lists it.
fn record_root(conn: &Connection, ids: &Ids, canon: &str) -> Result<()> {
    let cur = path_id::capture_file_id(canon)?;
    match root_row(conn, canon)? {
        Some((id, sid)) if id == cur => {
            if !is_capability_sid(&sid) {
                return Err(FenceHelperError::msg(format!(
                    "the state database holds '{sid}' as the capability SID of '{canon}'"
                )));
            }
        }
        prior => {
            if prior.is_some() {
                // The path names another object than the one granted:
                // take the grant off that one first.
                forget_one(conn, ids, canon, false)?;
            }
            let sid = new_capability_sid()?;
            conn.execute(
                "INSERT INTO persistent_roots (canonical_path, file_id, capability_sid) \
                 VALUES (?1, ?2, ?3)",
                params![canon, cur.as_bytes().as_slice(), sid],
            )
            .with_context(|| format!("store the capability SID of '{canon}'"))?;
        }
    }
    // Before any ACE is written - see the module doc.
    if let Some(g) = &ids.group_sid {
        marker_set_group_sid(g)?;
    }
    marker_add(canon, cur).with_context(|| format!("list '{canon}' in the persistent-grant marker"))
}

/// A kept deny on a multi-link file is refused, as a per-session one
/// is: the links share one DACL and the rows are keyed by path.
fn refuse_hardlinked_deny(canon: &str, ace: SbAce, links: u32, is_dir: bool) -> Result<()> {
    if matches!(ace, SbAce::Deny(_)) && !is_dir && links > 1 {
        return Err(FenceHelperError::msg(format!(
            "deny refused: '{canon}' has {links} hardlink(s); the deny \
             would be kept or removed through one name only"
        )));
    }
    Ok(())
}

fn record_deny(conn: &Connection, canon: &str, ace: SbAce, root: &str) -> Result<()> {
    let (id, links, is_dir) = path_id::capture_id_and_links(canon)?;
    refuse_hardlinked_deny(canon, ace, links, is_dir)?;
    let prior: Option<(String, Vec<u8>)> = conn
        .query_row(
            "SELECT mask, file_id FROM persistent_denies \
             WHERE canonical_path = ?1 AND kind = ?2",
            params![canon, ace.kind()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .context("SELECT persistent_denies")?;
    // Never narrowed while the object is the same: a deny stays as
    // wide as any session asked for until the root is forgotten.
    let eff = match prior {
        Some((mask, old)) if old.as_slice() == id.as_bytes() => SbAce::parse(ace.kind(), &mask)?.max(ace),
        _ => ace,
    };
    conn.execute(
        "INSERT INTO persistent_denies (canonical_path, kind, file_id, mask, root) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(canonical_path, kind) DO UPDATE SET \
           file_id = excluded.file_id, mask = excluded.mask, root = excluded.root",
        params![canon, ace.kind(), id.as_bytes().as_slice(), eff.as_str(), root],
    )
    .context("UPSERT persistent_denies")?;
    Ok(())
}

/// Session start with the write-restricted token: record `roots` and
/// the deny targets under them, then converge every path, roots
/// first. Deny targets under no root are returned for per-session use.
pub fn apply(
    db: &Locked,
    ids: &Ids,
    roots: &[String],
    denies: &[(String, SbAce)],
) -> Result<(Applied, Vec<(String, SbAce)>)> {
    let conn = db.conn();
    if ids.group_sid.is_none() {
        return Err(FenceHelperError::msg(
            "no install record: run `fence-helper install`".to_string(),
        ));
    }
    // A kept grant whose volume was absent when it was forgotten: if
    // the volume is back, its ACEs come off now.
    retry_pending(conn, ids, false)?;
    let mut touched: Vec<String> = Vec::new();
    let mut touch = |p: &str| {
        if !touched.iter().any(|t| t == p) {
            touched.push(p.to_owned());
        }
    };
    for root in roots {
        record_root(conn, ids, root)?;
        touch(root);
    }
    let all_roots = all_root_paths(conn)?;
    let mut per_session = Vec::new();
    for (canon, ace) in denies {
        let Some(root) = covering_root(&all_roots, canon) else {
            per_session.push((canon.clone(), *ace));
            continue;
        };
        record_deny(conn, canon, *ace, root)?;
        touch(canon);
        if matches!(ace, SbAce::Deny(_) | SbAce::DenyDelete)
            && let Some(parent) = path_id::canonical_parent_of(canon)
        {
            record_deny(conn, &parent, SbAce::DenyFdc, root)?;
            touch(&parent);
        }
    }
    let mut out = Applied {
        roots: roots.len(),
        ..Default::default()
    };
    let before = acl::write_counts();
    for p in &touched {
        if !converge(conn, ids, p).with_context(|| format!("persistent grant '{p}'"))? {
            out.unchanged += 1;
        }
    }
    let after = acl::write_counts();
    out.writes = after.0 - before.0;
    out.local_writes = after.1 - before.1;
    Ok((out, per_session))
}

/// The capability SIDs of `roots`, for the child token. A root with
/// none recorded for the object now at its path is an error: the
/// command must not run with a shorter list than asked.
pub fn capability_sids_for(roots: &[String]) -> Result<Vec<String>> {
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    let conn = crate::state_db::open_db_ro()?
        .ok_or_else(|| FenceHelperError::msg("the state database is missing or unreadable".to_string()))?;
    let mut out = Vec::with_capacity(roots.len());
    for raw in roots {
        let (canon, _) =
            path_id::canonicalize_path(raw).map_err(|e| FenceHelperError::msg(format!("write root '{raw}': {e}")))?;
        let cur = path_id::capture_file_id(&canon)?;
        match root_row(&conn, &canon)? {
            Some((id, sid)) if id == cur && is_capability_sid(&sid) => {
                if !out.contains(&sid) {
                    out.push(sid);
                }
            }
            _ => {
                return Err(FenceHelperError::msg(format!(
                    "write root '{canon}' has no persistent grant recorded for it - \
                 `fence-helper acl grant --persistent` it first"
                )));
            }
        }
    }
    Ok(out)
}

// ─── Forget ─────────────────────────────────────────────────────────

/// Per-root outcome of [`forget`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forgot {
    /// Persistent ACEs were on disk and were removed.
    Removed,
    /// Nothing was on disk; the records are gone.
    Absent,
    /// The object is on a volume that is not mounted. Its records are
    /// gone and it is listed as a pending strip, to be removed when
    /// the volume is back.
    Pending,
    /// Could not be completed; the root stays recorded.
    Failed(String),
    /// `--force` only: the object could not be located; its record is
    /// gone and its ACEs may still be on disk.
    Dropped,
}

impl Forgot {
    pub fn as_str(&self) -> &'static str {
        match self {
            Forgot::Removed => "removed",
            Forgot::Absent => "absent",
            Forgot::Pending => "pending",
            Forgot::Failed(_) => "failed",
            Forgot::Dropped => "dropped",
        }
    }
}

/// What opening a recorded path found, as far as it decides what
/// becomes of the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtPath {
    /// The recorded object.
    Recorded,
    /// Something, but not the recorded object.
    Another,
    /// Nothing has that name ([`path_id::OpenFailure::NotFound`]).
    Nothing,
    /// The volume cannot be reached right now
    /// ([`path_id::OpenFailure::VolumeUnavailable`]).
    Unavailable,
}

/// What removing a record's ACEs takes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Disposition {
    /// Remove them at the recorded path.
    StripHere,
    /// The object is at another path now - renamed, moved, or the
    /// volume is mounted elsewhere: remove them there.
    StripAt(String),
    /// The object no longer exists: there is nothing to remove.
    Drop,
    /// The object is on a volume that cannot be reached now: a
    /// pending strip.
    Pending,
    /// It could not be established where the object is while its
    /// volume is there: an error; the record stays.
    Fail,
}

/// What becomes of a record, by where its path leads now and where the
/// object was found by file id (asked only when the path does not lead
/// to it). On a share (`network`) a volume cannot be looked for.
fn disposition(at_path: AtPath, located: impl FnOnce() -> Located, network: bool) -> Disposition {
    if at_path == AtPath::Recorded {
        return Disposition::StripHere;
    }
    match (located(), at_path) {
        (Located::At(p), _) => Disposition::StripAt(p),
        (Located::Gone, _) => Disposition::Drop,
        (Located::VolumeAbsent | Located::Unknown, AtPath::Unavailable) => Disposition::Pending,
        (Located::VolumeAbsent, _) if !network => Disposition::Pending,
        _ => Disposition::Fail,
    }
}

/// Whether `canon` is somewhere volumes cannot be enumerated: a share,
/// or a drive letter mapped to one.
fn on_network(canon: &str) -> bool {
    !path_id::is_drive_path(canon) || path_id::on_network_drive(canon)
}

/// What is at `path` now, against the recorded object `id`: the open
/// error classified ([`path_id::open_failure`]). An error that is
/// neither "nothing there" nor "volume unavailable" is returned.
fn at_path(path: &str, id: &FileId) -> Result<AtPath> {
    match path_id::capture_file_id(path) {
        Ok(now) if now == *id => Ok(AtPath::Recorded),
        Ok(_) => Ok(AtPath::Another),
        Err(e) => match path_id::open_failure(&e) {
            path_id::OpenFailure::NotFound => Ok(AtPath::Nothing),
            path_id::OpenFailure::VolumeUnavailable => Ok(AtPath::Unavailable),
            path_id::OpenFailure::Other => Err(e.context(format!("open '{path}'"))),
        },
    }
}

/// A granted object that cannot be located fails the forget; with
/// `force` it is reported and skipped, and the root's record is dropped.
fn unlocatable(canon: &str, force: bool) -> Result<()> {
    if !force {
        return Err(FenceHelperError::msg(format!(
            "the object '{canon}' was granted on is no longer at that path and could \
             not be located by file id; its ACEs may still be on disk (`acl forget \
             --all --force` drops its record)"
        )));
    }
    eprintln!(
        "fence-helper: acl forget --force: the object '{canon}' was granted on could not \
         be located; its record is dropped and its ACEs may still be on disk"
    );
    Ok(())
}

/// Remove `canon`'s persistent grant. Rows are deleted only after every
/// DACL write succeeded; an object whose volume is away becomes pending,
/// and one that cannot be located fails the call unless `force`.
pub(crate) fn forget_one(conn: &Connection, ids: &Ids, canon: &str, force: bool) -> Result<Forgot> {
    forget_one_with(conn, ids, canon, force, path_id::locate_by_file_id)
}

fn forget_one_with(
    conn: &Connection,
    ids: &Ids,
    canon: &str,
    force: bool,
    locate: impl Fn(&FileId) -> Located,
) -> Result<Forgot> {
    // Every object the grant may have been written on.
    let mut granted: Vec<FileId> = Vec::new();
    granted.extend(root_row(conn, canon)?.map(|(id, _)| id));
    for r in read_marker()?.roots {
        if r.path == canon && !granted.contains(&r.file_id) {
            granted.push(r.file_id);
        }
    }
    // Durable before the walk: the transaction below takes its own
    // writes back if the process is killed, the object's DACL stays.
    mark_strip_begun(conn, canon)?;
    let tx = conn.unchecked_transaction().context("begin forget tx")?;
    let denies: Vec<(String, Vec<u8>)> = {
        let mut s = tx.prepare("SELECT DISTINCT canonical_path, file_id FROM persistent_denies WHERE root = ?1")?;
        let rows = s.query_map(params![canon], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    tx.execute("DELETE FROM persistent_denies WHERE root = ?1", params![canon])?;
    tx.execute("DELETE FROM persistent_roots WHERE canonical_path = ?1", params![canon])?;
    // With the rows gone, converging a path strips it (or leaves
    // what another root still records for it).
    let mut removed = false;
    let mut dropped = false;
    let mut pending: Vec<PendingStrip> = Vec::new();
    let mut recorded: Vec<(String, FileId)> = granted.iter().map(|id| (canon.to_owned(), *id)).collect();
    for (d, id) in &denies {
        let id =
            FileId::from_bytes(id).with_context(|| format!("the recorded deny on '{d}' has an unreadable file id"))?;
        recorded.push((d.clone(), id));
    }
    // Whatever is at a recorded path now carries nothing of ours any
    // more, whichever object it is.
    let mut stripped_here: Vec<&str> = Vec::new();
    for (path, id) in &recorded {
        let at = at_path(path, id)?;
        if matches!(at, AtPath::Recorded | AtPath::Another) && !stripped_here.contains(&path.as_str()) {
            removed |= converge(&tx, ids, path)?;
            stripped_here.push(path);
        }
        match disposition(at, || locate(id), on_network(path)) {
            Disposition::StripHere | Disposition::Drop => {}
            Disposition::StripAt(now) => removed |= converge(&tx, ids, &now)?,
            Disposition::Pending => pending.push(PendingStrip {
                file_id: *id,
                group_sid: ids.group_sid.clone().unwrap_or_default(),
                path: path.clone(),
            }),
            Disposition::Fail => {
                unlocatable(path, force)?;
                dropped = true;
            }
        }
    }
    // A root with no record of the object it was granted on (nothing
    // in the database or the marker): the path is all there is.
    if granted.is_empty() && path_id::capture_file_id(canon).is_ok() {
        removed |= converge(&tx, ids, canon)?;
    }
    // Before the rows go: a crash in between leaves the object listed
    // twice, never not at all.
    marker_add_pending(&pending)?;
    tx.commit().context("commit forget")?;
    marker_remove(canon)?;
    Ok(if dropped {
        Forgot::Dropped
    } else if !pending.is_empty() {
        Forgot::Pending
    } else if removed {
        Forgot::Removed
    } else {
        Forgot::Absent
    })
}

/// Look at every pending strip again; an entry goes only once its ACEs are
/// off or its volume says the object is gone. A share is contacted only
/// with `probe_network`. Returns what is still pending.
pub(crate) fn retry_pending(conn: &Connection, ids: &Ids, probe_network: bool) -> Result<Vec<PendingStrip>> {
    retry_pending_with(
        read_marker()?.pending,
        |p| {
            if !on_network(&p.path) {
                return path_id::locate_by_file_id_local(&p.file_id);
            }
            if !probe_network {
                return Located::VolumeAbsent;
            }
            // Not at its recorded path: looked for by id on that share. If the
            // share cannot say where it is or that it is gone, the entry stays.
            let elsewhere = || match path_id::locate_on_share(&p.path, &p.file_id) {
                Located::Gone => {
                    eprintln!(
                        "fence-helper: the object the kept grant on '{}' was made on no longer \
                         exists on its share; its record is dropped",
                        p.path
                    );
                    Located::Gone
                }
                Located::At(at) => Located::At(at),
                other => {
                    eprintln!(
                        "fence-helper: the object the kept grant on '{}' was made on is not at \
                         that path, and the share could not say where it is or that it is \
                         gone; if it was renamed it still carries the grant. The record \
                         stays (`fence-helper acl forget --drop-pending` gives it up).",
                        p.path
                    );
                    other
                }
            };
            match path_id::capture_file_id(&p.path) {
                Ok(now) if now == p.file_id => Located::At(p.path.clone()),
                Ok(_) => elsewhere(),
                Err(e) if path_id::is_not_found(&e) => elsewhere(),
                Err(_) => Located::VolumeAbsent,
            }
        },
        |p, at| {
            // The ACEs name the group that was installed when the grant
            // was made, which may not be the one installed now.
            let then = Ids {
                group_sid: Some(p.group_sid.clone()).filter(|g| !g.is_empty()),
                ..ids.clone()
            };
            converge(conn, &then, at).map(|_| ())
        },
    )
    .and_then(|(left, changed)| {
        if changed {
            write_pending(&left)?;
        }
        Ok(left)
    })
}

/// [`retry_pending`] on what was read, with the lookups passed in:
/// what stays pending, and whether the list changed.
fn retry_pending_with(
    pending: Vec<PendingStrip>,
    locate: impl Fn(&PendingStrip) -> Located,
    strip: impl Fn(&PendingStrip, &str) -> Result<()>,
) -> Result<(Vec<PendingStrip>, bool)> {
    let before = pending.len();
    let mut left = Vec::new();
    for p in pending {
        match locate(&p) {
            Located::At(at) => {
                if let Err(e) = strip(&p, &at) {
                    eprintln!(
                        "fence-helper: WARNING: the kept grant on '{at}' (forgotten while its \
                         volume was absent) could not be removed: {e:#}"
                    );
                    left.push(p);
                }
            }
            Located::Gone => {}
            Located::VolumeAbsent | Located::Unknown => left.push(p),
        }
    }
    let changed = left.len() != before;
    Ok((left, changed))
}

/// Give up the pending strips recorded for `paths` (a volume that will not
/// return). Refuses an entry whose volume is present; writes nothing but
/// the list.
pub fn drop_pending(conn: &Connection, ids: &Ids, paths: &[String]) -> Result<Vec<PendingStrip>> {
    let pending = retry_pending(conn, ids, true)?;
    let names = |p: &PendingStrip, want: &str| {
        let want = want.trim_end_matches('\\');
        let have = p.path.trim_end_matches('\\');
        have.eq_ignore_ascii_case(want) || have.strip_prefix(r"\\?\").is_some_and(|h| h.eq_ignore_ascii_case(want))
    };
    let named = |p: &PendingStrip| paths.iter().any(|want| names(p, want));
    for want in paths {
        if !pending.iter().any(|p| names(p, want)) {
            return Err(FenceHelperError::msg(format!(
                "no kept grant is waiting for a volume at '{want}' (what could be removed \
                 has been)"
            )));
        }
    }
    for p in pending.iter().filter(|p| named(p)) {
        // What is still pending on a share is unreachable or could not be
        // found by its id.
        let reachable = !on_network(&p.path) && path_id::volume_mounted(p.file_id.volume_serial);
        if reachable {
            return Err(FenceHelperError::msg(format!(
                "the volume of '{}' is present, and its entries could not be removed just \
                 now; they are live and are not given up. Run `fence-helper acl forget --all` \
                 for the reason",
                p.path
            )));
        }
    }
    let (dropped, left): (Vec<_>, Vec<_>) = pending.into_iter().partition(named);
    for p in dropped.iter().filter(|p| on_network(&p.path)) {
        // Not at its path and not found by id: which is not the share
        // saying it is gone.
        eprintln!(
            "fence-helper: giving up the kept grant recorded at '{}': the object is not at that \
             path and its share could not be asked where it is. If it still exists under \
             another name it keeps the entries of {}; `icacls <its path> /remove:g \"*{}\" \
             /T` removes them.",
            p.path,
            if p.group_sid.is_empty() {
                "the sandbox group"
            } else {
                "group"
            },
            if p.group_sid.is_empty() {
                "<sandbox group SID>"
            } else {
                &p.group_sid
            },
        );
    }
    write_pending(&left)?;
    Ok(dropped)
}

/// `acl forget`: remove the persistent grants of `roots`, or with
/// `all` of every root the marker or the database lists. `all` also
/// clears the marker's `EverEnabled` once no root is left listed.
pub fn forget(db: &Locked, ids: &Ids, roots: &[String], all: bool, force: bool) -> Result<Vec<(String, Forgot)>> {
    let conn = db.conn();
    // First the grants forgotten earlier whose volume was absent.
    // An explicit request: one on a share is tried at its path too.
    let still_pending = retry_pending(conn, ids, true)?;
    let mut targets: Vec<String> = roots.to_vec();
    if all {
        targets.extend(read_marker()?.roots.into_iter().map(|r| r.path));
        targets.extend(all_root_paths(conn)?);
    }
    let mut seen = std::collections::HashSet::new();
    targets.retain(|t| seen.insert(t.clone()));
    let mut out = Vec::with_capacity(targets.len());
    for t in targets {
        let r = match forget_one(conn, ids, &t, force) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("fence-helper: forget '{t}': {e:#}");
                Forgot::Failed(format!("{e:#}"))
            }
        };
        out.push((t, r));
    }
    // … which are part of the answer for as long as they wait.
    for p in still_pending {
        if !out.iter().any(|(t, _)| *t == p.path) {
            out.push((p.path, Forgot::Pending));
        }
    }
    if all {
        marker_clear_ever_if_unused(conn)?;
    }
    Ok(out)
}

/// `fence-helper uninstall`: forget every persistent grant of the calling
/// user. Does nothing - and creates no state - when none was ever
/// made. Returns how many roots were processed and how many failed.
pub fn forget_everything() -> Result<Forgotten> {
    forget_everything_as(None)
}

/// What the half of `fence-helper uninstall` that runs as the calling user
/// does once the elevated half has returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterElevation {
    /// The install is gone: forget the caller's grants. `step_failed`
    /// is the elevated step's exit code if it was not 0 - it removed
    /// the account and then failed at something else.
    ForgetGrants { step_failed: Option<i32> },
    /// The install is still there: change nothing of the caller's,
    /// exit with this code.
    LeaveEverything(i32),
}

pub fn after_elevation(elevated_exit: i32, install_record_present: bool) -> AfterElevation {
    if install_record_present {
        // Not removed. An exit of 0 with the record still there is not
        // a clean removal either.
        AfterElevation::LeaveEverything(if elevated_exit == 0 { 1 } else { elevated_exit })
    } else {
        AfterElevation::ForgetGrants {
            step_failed: (elevated_exit != 0).then_some(elevated_exit),
        }
    }
}

/// What [`forget_everything`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Forgotten {
    /// Roots processed.
    pub roots: usize,
    /// Roots that could not be completed; they stay recorded.
    pub failed: usize,
    /// Paths on volumes that are not mounted, still to be stripped.
    pub pending: Vec<String>,
}

/// [`forget_everything`] with the identities read earlier: after the
/// install record is gone, `ids` taken before it went still name the
/// group the grants were written for.
pub fn forget_everything_as(ids: Option<&Ids>) -> Result<Forgotten> {
    if read_marker().is_ok_and(|m| m.nothing_recorded()) {
        return Ok(Forgotten::default());
    }
    let loaded;
    let ids = match ids {
        Some(ids) => ids,
        None => {
            loaded = Ids::load()?;
            &loaded
        }
    };
    let holder = crate::state_db::HolderPid(std::process::id());
    let (out, _) = crate::state_db::with_init_lock(holder, false, |db| forget(db, ids, &[], true, false))?;
    Ok(Forgotten {
        roots: out.len(),
        failed: out.iter().filter(|(_, r)| matches!(r, Forgot::Failed(_))).count(),
        pending: out
            .into_iter()
            .filter(|(_, r)| *r == Forgot::Pending)
            .map(|(p, _)| p)
            .collect(),
    })
}

/// For `acl grant` without `--persistent`: refuse while a persistent
/// grant is recorded or a target's DACL cannot be read, and remove a
/// persistent ALLOW found on a target's own DACL (inherited: refuse).
pub fn clear_for_unrestricted_grant(db: &Locked, targets: &[String]) -> Result<()> {
    // A grant forgotten while its volume was absent, and back now.
    retry_pending(db.conn(), &Ids::load()?, false)?;
    if let Some(why) = write_grant_block_reason() {
        return Err(FenceHelperError::msg(format!(
            "{why}; grants for the ordinary token are refused until `fence-helper acl forget \
             --all` has removed them"
        )));
    }
    if let Some(why) = crate::profile::block_reason() {
        return Err(FenceHelperError::msg(format!(
            "{why}; grants for the ordinary token are refused until `fence-helper profile \
             forget` has removed it"
        )));
    }
    let ids = Ids::load()?;
    for canon in targets {
        match unrecorded_grant_on(acl::read_aces(canon), ids.group_sid.as_deref()) {
            Ok(UnrecordedGrant::None) => {}
            Ok(UnrecordedGrant::Own) => {
                eprintln!("fence-helper: '{canon}' carries an unrecorded persistent grant; removing it");
                forget_one(db.conn(), &ids, canon, false)
                    .with_context(|| format!("remove the persistent grant on '{canon}'"))?;
            }
            Ok(UnrecordedGrant::Inherited) => {
                return Err(FenceHelperError::msg(format!(
                    "'{canon}' inherits a persistent write grant from a directory above it that \
                 is no longer recorded; run `fence-helper acl forget <that directory>`"
                )));
            }
            Err(e) => {
                return Err(FenceHelperError::msg(format!(
                    "the access list of '{canon}' cannot be read ({e:#}), so a persistent write \
                 grant on it cannot be ruled out; grants for the ordinary token are refused"
                )));
            }
        }
    }
    Ok(())
}

/// A persistent ALLOW found on a path that no record lists.
#[derive(Debug, PartialEq, Eq)]
enum UnrecordedGrant {
    None,
    /// Written on the path itself: `forget_one` can remove it.
    Own,
    /// Inherited from a directory above: not removable from here.
    Inherited,
}

/// What `aces` (a path's own DACL) says about a persistent ALLOW. A
/// DACL that could not be read is an error, never
/// [`UnrecordedGrant::None`].
fn unrecorded_grant_on(aces: Result<Vec<AceView>>, group_sid: Option<&str>) -> Result<UnrecordedGrant> {
    let aces = aces?;
    let mut grants = aces
        .iter()
        .filter(|a| !a.deny && (group_sid == Some(a.sid.as_str()) || is_capability_sid(&a.sid)))
        .peekable();
    if grants.peek().is_none() {
        return Ok(UnrecordedGrant::None);
    }
    Ok(if grants.any(|a| a.inherited) {
        UnrecordedGrant::Inherited
    } else {
        UnrecordedGrant::Own
    })
}

#[cfg(test)]
mod tests;
