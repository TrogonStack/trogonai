//! The pure half of the profile read grant ([`crate::profile`]): what is
//! never readable, and which directory gets which entry. No Windows call
//! is made here.
//! The walk goes down from the profile root. A directory is a container
//! (a non-inheriting list entry; children looked at one by one) exactly
//! when some exclusion could lie beneath it, existing or not; otherwise it
//! is granted whole. So no excluded path ever has an ancestor that carries
//! an inheriting allow. A protected exclusion is read-denied; an
//! ungranted-only one (the temporary directory) gets nothing at all. A
//! link at or above a rule's path is resolved, and the rule is applied
//! beneath its target as well ([`plan_resolved`]).

/// Version of the exclusion list and of the walk rule. A recorded
/// grant made with another version is reported as outdated.
pub const LIST_VERSION: u32 = 3;

/// What a built-in exclusion's path is relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// The profile directory.
    Profile,
    /// The roaming application-data known folder.
    Roaming,
    /// The local application-data known folder.
    Local,
}

/// One built-in exclusion. `path` uses `\`; a component that is `*`
/// matches any one name, and a component ending in `*` any name with
/// that prefix.
#[derive(Debug, Clone, Copy)]
pub struct Excl {
    pub base: Base,
    pub path: &'static str,
    /// A directory (denied with an inheriting entry, and created empty
    /// when missing) or a file.
    pub dir: bool,
    /// False for a directory whose deny is written by its owner in
    /// this crate; it is then only kept out of the grant.
    pub stamp: bool,
    /// Ungranted-only (see the module doc): kept out of the grant,
    /// not denied. Every other entry is protected.
    pub ungranted_only: bool,
    /// Create the directory, empty, when it is missing, so the deny
    /// has an object to sit on.
    pub create: bool,
}

/// A protected directory.
const fn dir(base: Base, path: &'static str) -> Excl {
    Excl {
        base,
        path,
        dir: true,
        stamp: true,
        ungranted_only: false,
        create: true,
    }
}

/// A protected directory that is not created when missing.
const fn store(base: Base, path: &'static str) -> Excl {
    Excl {
        base,
        path,
        dir: true,
        stamp: true,
        ungranted_only: false,
        create: false,
    }
}

/// A protected file.
const fn file(base: Base, path: &'static str) -> Excl {
    Excl {
        base,
        path,
        dir: false,
        stamp: true,
        ungranted_only: false,
        create: false,
    }
}

/// An ungranted-only directory.
const fn ungranted(base: Base, path: &'static str) -> Excl {
    Excl {
        base,
        path,
        dir: true,
        stamp: false,
        ungranted_only: true,
        create: false,
    }
}

use Base::{Local, Profile, Roaming};
use NoDestination::{Elsewhere, Malformed};

/// Every path that is never granted and carries a read deny for the
/// sandbox group. Configuration can add to this list
/// ([`Rules::new`]'s `extra`); nothing removes an entry.
pub const BUILT_IN: &[Excl] = &[
    // Keys and sign-ins kept in dot-directories of the profile.
    dir(Profile, ".ssh"),
    dir(Profile, ".gnupg"),
    dir(Profile, ".aws"),
    dir(Profile, ".azure"),
    dir(Profile, ".kube"),
    dir(Profile, ".docker"),
    dir(Profile, ".terraform.d"),
    dir(Profile, ".wrangler"),
    dir(Profile, ".vercel"),
    dir(Profile, ".netlify"),
    dir(Profile, ".claude"),
    // The same kind of store under the XDG-style `.config`, which
    // tools from other platforms use on Windows too.
    dir(Profile, r".config\gcloud"),
    dir(Profile, r".config\gh"),
    dir(Profile, r".config\configstore"),
    file(Profile, r".config\git\credentials"),
    // Where those tools keep the same sign-ins when they follow the
    // Windows convention instead.
    dir(Roaming, "gcloud"),
    dir(Roaming, "GitHub CLI"),
    dir(Roaming, "gnupg"),
    dir(Roaming, "com.vercel.cli"),
    dir(Roaming, "netlify"),
    dir(Roaming, "xdg.config"),
    dir(Roaming, "terraform.d"),
    file(Roaming, "terraform.rc"),
    dir(Local, "heroku"),
    // Plaintext token files of further command-line tools, at their
    // Windows locations and under `.config` where the tool follows XDG.
    store(Local, "github-copilot"),
    store(Profile, r".config\github-copilot"),
    store(Profile, r".config\doctl"),
    store(Roaming, "doctl"),
    store(Profile, r".config\op"),
    file(Profile, r".config\hub"),
    file(Profile, r".gem\credentials"),
    file(Profile, r".pulumi\credentials.json"),
    file(Roaming, r"postgresql\pgpass.conf"),
    file(Profile, ".pgpass"),
    file(Roaming, r"Composer\auth.json"),
    file(Profile, r".composer\auth.json"),
    file(Profile, r".fly\config.yml"),
    file(Profile, ".databrickscfg"),
    file(Profile, ".boto"),
    file(Profile, ".s3cfg"),
    file(Roaming, "s3cmd.ini"),
    store(Profile, ".kaggle"),
    store(Profile, ".snowsql"),
    store(Profile, ".oci"),
    store(Profile, ".bluemix"),
    store(Profile, ".ibmcloud"),
    store(Profile, ".railway"),
    store(Profile, ".supabase"),
    // Package-feed sign-in caches (the token stores, not the
    // configuration files that name a feed, which are readable).
    dir(Local, ".IdentityService"),
    dir(Local, "MicrosoftCredentialProvider"),
    // Credential files.
    file(Profile, ".git-credentials"),
    file(Profile, ".netrc"),
    file(Profile, "_netrc"),
    file(Profile, ".pypirc"),
    file(Profile, ".vault-token"),
    file(Profile, ".terraformrc"),
    file(Profile, r".cargo\credentials"),
    file(Profile, r".cargo\credentials.toml"),
    file(Profile, r".gradle\enterprise\keys.properties"),
    file(Profile, r".gradle\develocity\keys.properties"),
    // The user's registry hives and their logs.
    file(Profile, "NTUSER.DAT*"),
    file(Profile, "ntuser.ini"),
    file(Profile, "ntuser.pol"),
    file(Local, r"Microsoft\Windows\UsrClass.dat*"),
    // Browser profiles: cookies, saved passwords, session storage.
    dir(Local, r"Google\Chrome\User Data"),
    dir(Local, r"Microsoft\Edge\User Data"),
    dir(Local, r"BraveSoftware\Brave-Browser\User Data"),
    dir(Roaming, r"Mozilla\Firefox"),
    dir(Local, r"Mozilla\Firefox"),
    // Other channels of those browsers, others on the same engines,
    // and the mail client that shares Firefox's profile layout.
    store(Local, r"Google\Chrome Beta\User Data"),
    store(Local, r"Google\Chrome Dev\User Data"),
    store(Local, r"Google\Chrome SxS\User Data"),
    store(Local, r"Chromium\User Data"),
    store(Local, r"Microsoft\Edge Beta\User Data"),
    store(Local, r"Microsoft\Edge Dev\User Data"),
    store(Local, r"Microsoft\Edge SxS\User Data"),
    store(Local, r"BraveSoftware\Brave-Browser-Beta\User Data"),
    store(Local, r"BraveSoftware\Brave-Browser-Nightly\User Data"),
    store(Local, r"Vivaldi\User Data"),
    store(Local, r"Yandex\YandexBrowser\User Data"),
    store(Roaming, "Opera Software"),
    store(Local, "Opera Software"),
    store(Roaming, r"Mozilla\SeaMonkey"),
    store(Roaming, "Thunderbird"),
    store(Local, "Thunderbird"),
    // Embedded browser profiles of desktop applications.
    dir(Local, r"*\EBWebView"),
    dir(Local, r"Microsoft\*\EBWebView"),
    // Password managers.
    dir(Roaming, "1Password"),
    dir(Local, "1Password"),
    dir(Roaming, "Bitwarden"),
    dir(Roaming, "Bitwarden CLI"),
    dir(Roaming, "KeePass"),
    dir(Roaming, "KeePassXC"),
    // Windows' own per-user secret stores.
    dir(Roaming, r"Microsoft\Credentials"),
    dir(Roaming, r"Microsoft\Protect"),
    dir(Roaming, r"Microsoft\Vault"),
    dir(Roaming, r"Microsoft\Crypto"),
    dir(Roaming, r"Microsoft\SystemCertificates"),
    dir(Local, r"Microsoft\Credentials"),
    dir(Local, r"Microsoft\Protect"),
    dir(Local, r"Microsoft\Vault"),
    dir(Local, r"Microsoft\Crypto"),
    dir(Local, r"Microsoft\SystemCertificates"),
    // Account token caches and web sessions of the signed-in user.
    dir(Local, r"Microsoft\IdentityCache"),
    dir(Local, r"Microsoft\TokenBroker"),
    dir(Local, r"Microsoft\OneAuth"),
    dir(Local, r"Microsoft\Windows\INetCookies"),
    dir(Local, r"Microsoft\Windows\INetCache"),
    dir(Local, r"Microsoft\Windows\WebCache"),
    // Private data of packaged applications.
    dir(Local, "Packages"),
    // This library's state directory. Its owner (`state_db`) writes its
    // access list, deny included, on every open.
    Excl {
        base: Local,
        path: "trogon-sandbox",
        dir: true,
        stamp: false,
        ungranted_only: false,
        create: false,
    },
    // Ungranted-only: every program writes there, and sessions stage
    // directories of their own in it.
    ungranted(Local, "Temp"),
];

/// Files in the profile ROOT that are granted by name. Every other
/// file of the root is left unreadable. These can hold the sign-in
/// of a private package feed or a git host.
pub const READABLE_ROOT_FILES: &[&str] = &[".npmrc", ".gitconfig"];

/// The configuration files that are readable by design although they
/// can hold a feed sign-in, with what reads them.
pub const READABLE_CONFIG: &[(Base, &str, &str)] = &[
    (Profile, ".npmrc", "npm"),
    (Profile, ".gitconfig", "git"),
    (Profile, r".config\git\config", "git"),
    (Roaming, r"pip\pip.ini", "pip"),
    (Profile, r"pip\pip.ini", "pip"),
    (Profile, r".config\pip\pip.conf", "pip"),
    (Roaming, r"NuGet\NuGet.Config", "NuGet"),
    (Profile, r".m2\settings.xml", "Maven"),
    (Profile, r".m2\settings-security.xml", "Maven"),
    (Profile, r".gradle\gradle.properties", "Gradle"),
];

// ─── Names ──────────────────────────────────────────────────────────

/// A file name in the form names are compared in: without an
/// alternate-data-stream suffix (`name:stream`), without the trailing
/// dots and spaces Win32 drops when it opens a name, lower-cased.
pub fn norm_component(name: &str) -> String {
    let base = name.split(':').next().unwrap_or("");
    base.trim_end_matches(['.', ' ']).to_lowercase()
}

/// One component of an exclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pat {
    Lit(String),
    Prefix(String),
    Any,
}

impl Pat {
    fn parse(component: &str) -> Pat {
        if component == "*" {
            Pat::Any
        } else if let Some(prefix) = component.strip_suffix('*') {
            Pat::Prefix(norm_component(prefix))
        } else {
            Pat::Lit(norm_component(component))
        }
    }

    /// Whether a normalized name matches. The empty name (`.`, `..`,
    /// or a name of dots and spaces only) matches nothing.
    fn matches(&self, norm: &str) -> bool {
        if norm.is_empty() {
            return false;
        }
        match self {
            Pat::Lit(l) => l == norm,
            Pat::Prefix(p) => norm.starts_with(p.as_str()),
            Pat::Any => true,
        }
    }
}

/// A directory entry's names: the long one and, where the volume
/// keeps them, the 8.3 one. An exclusion matches if either does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub long: String,
    pub short: Option<String>,
}

impl Name {
    pub fn long(name: &str) -> Name {
        Name {
            long: name.to_owned(),
            short: None,
        }
    }

    fn matched_by(&self, pat: &Pat) -> bool {
        pat.matches(&norm_component(&self.long))
            || self.short.as_deref().is_some_and(|s| pat.matches(&norm_component(s)))
    }
}

/// `path` split into components, either separator, with a `\\?\`
/// prefix and empty components dropped.
pub fn split_path(path: &str) -> Vec<String> {
    let p = path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix("//?/"))
        .unwrap_or(path);
    p.split(['\\', '/'])
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The components of `path` beneath `profile`, or why it cannot be an
/// exclusion: it is not inside the profile, or it holds `.`, `..` or
/// a wildcard.
pub fn rel_components(profile: &str, path: &str) -> Result<Vec<String>, String> {
    let comps = split_path(path);
    let absolute = path.starts_with(['\\', '/']) || comps.first().is_some_and(|c| c.ends_with(':'));
    let rel = if absolute {
        let root = split_path(profile);
        let inside = comps.len() >= root.len()
            && root
                .iter()
                .zip(&comps)
                .all(|(a, b)| a.to_lowercase() == b.to_lowercase());
        if !inside {
            return Err(format!("'{path}' is not inside the profile '{profile}'"));
        }
        comps[root.len()..].to_vec()
    } else {
        comps
    };
    for c in &rel {
        if c == "." || c == ".." || c.contains(['*', '?']) || norm_component(c).is_empty() {
            return Err(format!("'{path}': component '{c}' cannot be part of an exclusion"));
        }
    }
    Ok(rel)
}

// ─── Rules ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Rule {
    pats: Vec<Pat>,
    /// The components as spelled, for creating a missing directory.
    spelled: Vec<String>,
    stamp: bool,
    /// Ungranted-only; otherwise protected.
    ungranted: bool,
    /// A protected built-in directory with no wildcard: created when
    /// missing.
    placeholder: bool,
}

/// The exclusions in force for one profile: the built-in list placed
/// at the profile's known-folder locations, plus what configuration
/// added.
#[derive(Debug, Clone)]
pub struct Rules {
    rules: Vec<Rule>,
}

/// Where the application-data folders are when nothing redirects
/// them.
const CONVENTIONAL_ROAMING: [&str; 2] = ["AppData", "Roaming"];
const CONVENTIONAL_LOCAL: [&str; 2] = ["AppData", "Local"];

impl Rules {
    /// `roaming` and `local` are the known-folder locations as
    /// components beneath the profile, `None` when a folder is not
    /// inside the profile.
    pub fn new(roaming: Option<&[String]>, local: Option<&[String]>, extra: &[Vec<String>]) -> Rules {
        let mut rules = Vec::new();
        let conventional = |c: &[&str]| c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        for e in BUILT_IN {
            let mut bases: Vec<Vec<String>> = Vec::new();
            match e.base {
                Base::Profile => bases.push(Vec::new()),
                Base::Roaming => {
                    bases.push(conventional(&CONVENTIONAL_ROAMING));
                    bases.extend(roaming.map(<[String]>::to_vec));
                }
                Base::Local => {
                    bases.push(conventional(&CONVENTIONAL_LOCAL));
                    bases.extend(local.map(<[String]>::to_vec));
                }
            }
            bases.dedup_by(|a, b| {
                a.len() == b.len()
                    && a.iter()
                        .zip(b.iter())
                        .all(|(x, y)| norm_component(x) == norm_component(y))
            });
            for base in bases {
                let mut spelled = base;
                spelled.extend(e.path.split('\\').map(str::to_owned));
                let pats: Vec<Pat> = spelled.iter().map(|c| Pat::parse(c)).collect();
                let wild = pats.iter().any(|p| !matches!(p, Pat::Lit(_)));
                rules.push(Rule {
                    pats,
                    spelled,
                    stamp: e.stamp,
                    ungranted: e.ungranted_only,
                    placeholder: e.dir && e.stamp && e.create && !wild && !e.ungranted_only,
                });
            }
        }
        for x in extra.iter().filter(|x| !x.is_empty()) {
            rules.push(Rule {
                pats: x.iter().map(|c| Pat::Lit(norm_component(c))).collect(),
                spelled: x.clone(),
                stamp: true,
                ungranted: false,
                placeholder: false,
            });
        }
        Rules { rules }
    }

    /// The exclusions of one kind, as written: components beneath the
    /// profile joined by `\\`, wildcards included.
    fn spelled_where(&self, ungranted: bool) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in self.rules.iter().filter(|r| r.ungranted == ungranted) {
            let s = r.spelled.join("\\");
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    /// The protected exclusions, built-in and added.
    pub fn spelled(&self) -> Vec<String> {
        self.spelled_where(false)
    }

    /// The ungranted-only exclusions.
    pub fn spelled_ungranted(&self) -> Vec<String> {
        self.spelled_where(true)
    }

    fn at_or_beneath(&self, rel: &[Name], ungranted: bool) -> bool {
        self.rules.iter().any(|r| {
            r.ungranted == ungranted
                && r.pats.len() <= rel.len()
                && r.pats.iter().zip(rel).all(|(p, n)| n.matched_by(p))
        })
    }

    /// Whether `rel` (components beneath the profile) is a protected
    /// exclusion or lies beneath one.
    pub fn excludes(&self, rel: &[Name]) -> bool {
        self.at_or_beneath(rel, false)
    }

    /// Whether `rel` is an ungranted-only exclusion or lies beneath
    /// one.
    pub fn ungranted(&self, rel: &[Name]) -> bool {
        self.at_or_beneath(rel, true)
    }

    /// Whether an exclusion of either kind could lie strictly beneath
    /// the directory `rel`, whatever is on disk.
    pub fn could_exclude_beneath(&self, rel: &[Name]) -> bool {
        self.rules
            .iter()
            .any(|r| r.pats.len() > rel.len() && r.pats.iter().zip(rel).all(|(p, n)| n.matched_by(p)))
    }

    fn all(&self) -> Vec<usize> {
        (0..self.rules.len()).collect()
    }
}

// ─── Session grants against a recorded grant ────────────────────────

/// How a path stands to an exclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The path is the excluded one.
    Equal,
    /// It lies beneath the excluded directory.
    Inside,
    /// The excluded path lies beneath it.
    Above,
}

/// How `path` stands to the recorded exclusion `rule`, both absolute;
/// `rule` may hold the wildcards of the built-in list. `None` when
/// neither contains the other.
pub fn relation(path: &str, rule: &str) -> Option<Relation> {
    let p: Vec<String> = split_path(path).iter().map(|c| norm_component(c)).collect();
    let r: Vec<Pat> = split_path(rule).iter().map(|c| Pat::parse(c)).collect();
    let shared = p.len().min(r.len());
    if !(0..shared).all(|i| r[i].matches(&p[i])) {
        return None;
    }
    Some(match p.len().cmp(&r.len()) {
        std::cmp::Ordering::Equal => Relation::Equal,
        std::cmp::Ordering::Greater => Relation::Inside,
        std::cmp::Ordering::Less => Relation::Above,
    })
}

/// What listing a directory gave ([`session_grant_refusal`]'s `list`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    /// Its entry names.
    Entries(Vec<String>),
    /// Nothing to list: the path is not there, or is a file.
    Nothing,
    /// A link: not followed.
    Link,
    /// It is there and could not be listed - access denied, an I/O
    /// error, a path that is not looked at at all - with the reason.
    Failed(String),
}

/// What lies beneath a path that is above a wildcard rule.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Beneath {
    /// Every level that could lead to a match was listed; none does.
    NoMatch,
    /// Something that exists matches the rule.
    Match,
    /// A level could not be listed. Treated as a possible match: a
    /// path is admitted because it was looked at and found empty of
    /// matches, never because looking failed.
    Unexamined(String),
}

/// Whether something that exists beneath the directory `path` matches the
/// rest of `rule`, which `path` lies above. A directory that cannot be
/// listed counts as a match.
fn matched_beneath(path: &str, rule: &str, list: &dyn Fn(&str) -> Listing) -> Beneath {
    fn go(dir: &str, rest: &[Pat], top: bool, list: &dyn Fn(&str) -> Listing) -> Beneath {
        let Some((first, rest)) = rest.split_first() else {
            return Beneath::Match;
        };
        let names = match list(dir) {
            Listing::Entries(names) => names,
            Listing::Nothing => return Beneath::NoMatch,
            Listing::Link if top => return Beneath::Unexamined(format!("'{dir}' is a link")),
            Listing::Link => return Beneath::NoMatch,
            Listing::Failed(why) => {
                return Beneath::Unexamined(format!("'{dir}' could not be listed: {why}"));
            }
        };
        let mut unexamined = None;
        for name in names.iter().filter(|n| first.matches(&norm_component(n))) {
            let child = format!("{}\\{name}", dir.trim_end_matches('\\'));
            match go(&child, rest, false, list) {
                Beneath::Match => return Beneath::Match,
                Beneath::NoMatch => {}
                u @ Beneath::Unexamined(_) => unexamined = unexamined.or(Some(u)),
            }
        }
        unexamined.unwrap_or(Beneath::NoMatch)
    }
    let depth = split_path(path).len();
    let pats: Vec<Pat> = split_path(rule).iter().map(|c| Pat::parse(c)).collect();
    if pats.len() <= depth {
        return Beneath::NoMatch;
    }
    go(path, &pats[depth..], true, list)
}

/// Why a session must not be granted `path` while a profile read grant
/// with these exclusions exists (`None`: it may): it is a protected path,
/// lies inside one, or lies above one. Only a read path can be `admitted`.
pub fn session_grant_refusal(
    path: &str,
    write: bool,
    protected: &[String],
    ungranted: &[String],
    admitted: &[String],
    list: &dyn Fn(&str) -> Listing,
) -> Option<String> {
    let what = if write { "write" } else { "read" };
    for u in ungranted {
        if relation(path, u) == Some(Relation::Equal) {
            return Some(format!(
                "a {what} grant on '{path}' would cover everything in it, and it is not \
                 granted as a whole; grant a directory inside it"
            ));
        }
    }
    let is_admitted = !write
        && admitted
            .iter()
            .any(|a| relation(path, a) == Some(Relation::Equal) && !a.contains('*'));
    for r in protected {
        match relation(path, r) {
            None => {}
            Some(Relation::Above) if r.contains('*') => match matched_beneath(path, r, list) {
                Beneath::NoMatch => {}
                Beneath::Match => {
                    return Some(format!(
                        "a {what} grant on '{path}' would be inherited by what the excluded \
                         '{r}' covers beneath it; choose a narrower directory"
                    ));
                }
                Beneath::Unexamined(why) => {
                    return Some(format!(
                        "'{path}' may lie above something '{r}' excludes from the profile \
                         read grant, and could not be examined ({why}); a {what} grant is \
                         made only on a path that was looked at"
                    ));
                }
            },
            Some(Relation::Above) => {
                return Some(format!(
                    "a {what} grant on '{path}' would be inherited by the excluded '{r}' \
                     beneath it if that is ever created again; choose a narrower directory"
                ));
            }
            Some(Relation::Equal | Relation::Inside) if is_admitted => {}
            Some(Relation::Equal) => {
                return Some(format!("'{path}' is excluded from reads of the profile"));
            }
            Some(Relation::Inside) => {
                return Some(format!(
                    "'{path}' lies inside '{r}', which is excluded from reads of the profile"
                ));
            }
        }
    }
    for u in ungranted {
        if relation(path, u) == Some(Relation::Above) {
            return Some(format!(
                "a {what} grant on '{path}' would cover everything in '{u}', which is not \
                 granted as a whole; choose a narrower directory"
            ));
        }
    }
    None
}

// ─── Plan ───────────────────────────────────────────────────────────

/// One directory entry as the walk sees it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: Name,
    pub is_dir: bool,
    /// Set for an entry nothing may be written on or through: a
    /// reparse point or a cloud placeholder.
    pub skip: Option<String>,
    /// The entry is a reparse point: something that may redirect to
    /// another place ([`Tree::resolve_link`]).
    pub link: bool,
}

/// Where a link leads, with every link on the way resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// Inside the profile: the target's components beneath it (empty
    /// for the profile directory itself).
    Inside(Vec<String>),
    /// Outside the profile, not a redirection at all, or a link whose
    /// own data cannot be decoded, with a word about it.
    Elsewhere(String),
}

/// Why a link has no destination to follow ([`link_destination`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NoDestination {
    /// It leads off this machine's drive letters, or is not a
    /// redirection: known not to be inside the profile.
    #[error("{0}")]
    Elsewhere(String),
    /// Its data cannot be made sense of.
    #[error("{0}")]
    Malformed(String),
}

/// What a reparse point says about where it leads, as read from the
/// link itself ([`parse_reparse`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkData {
    /// A junction, with its target in the object namespace
    /// (`\??\C:\dir`).
    Junction(String),
    /// A symbolic link, with its target as stored and whether that is
    /// relative to the link's directory.
    Symlink { target: String, relative: bool },
    /// Some other kind of reparse point (a cloud placeholder, an
    /// application link), by tag. Not a redirection to follow.
    Other(u32),
}

/// Read a reparse data buffer (`FSCTL_GET_REPARSE_POINT`).
pub fn parse_reparse(buf: &[u8]) -> Result<LinkData, String> {
    const TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const TAG_SYMLINK: u32 = 0xA000_000C;
    let u16_at = |at: usize| -> Result<usize, String> {
        buf.get(at..at + 2)
            .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
            .ok_or_else(|| "truncated reparse data".to_owned())
    };
    let tag = buf
        .get(0..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or("truncated reparse data")?;
    // Header: tag, data length, reserved.
    let names_at = match tag {
        TAG_MOUNT_POINT => 16,
        TAG_SYMLINK => 20,
        other => return Ok(LinkData::Other(other)),
    };
    let (off, len) = (u16_at(8)?, u16_at(10)?);
    let bytes = buf
        .get(names_at + off..names_at + off + len)
        .ok_or("truncated reparse data")?;
    let wide: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let target = String::from_utf16(&wide).map_err(|_| "a link target that is not valid UTF-16")?;
    Ok(if tag == TAG_MOUNT_POINT {
        LinkData::Junction(target)
    } else {
        let flags = buf
            .get(16..20)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or("truncated reparse data")?;
        LinkData::Symlink {
            target,
            relative: flags & 1 != 0,
        }
    })
}

/// Where the link at `link` (`\\?\X:\…`) leads, with `rest` - the
/// components that followed it - appended: a path on a drive letter,
/// in the same form.
pub fn link_destination(link: &str, data: &LinkData, rest: &[String]) -> Result<String, NoDestination> {
    let drive_of = |p: &str| -> Option<(char, String)> {
        let b = p.as_bytes();
        (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
            .then(|| (b[0] as char, p[3..].to_owned()))
    };
    let not_drive = || Elsewhere(format!("'{link}' is not a drive path"));
    let link_plain = link.strip_prefix(r"\\?\").ok_or_else(not_drive)?;
    let (link_drive, link_tail) = drive_of(link_plain).ok_or_else(not_drive)?;
    let absolute = |target: &str| -> Result<(char, String), NoDestination> {
        let plain = target
            .strip_prefix(r"\??\")
            .or_else(|| target.strip_prefix(r"\\?\"))
            .unwrap_or(target);
        drive_of(plain).ok_or_else(|| {
            Elsewhere(format!(
                "leads to '{target}', which is not on a drive of this machine; not followed"
            ))
        })
    };
    let (drive, tail) = match data {
        LinkData::Other(tag) => {
            return Err(Elsewhere(format!(
                "a reparse point of another kind ({tag:#x}); not followed"
            )));
        }
        LinkData::Junction(target) => absolute(target)?,
        LinkData::Symlink {
            target,
            relative: false,
        } => absolute(target)?,
        LinkData::Symlink { target, relative: true } => {
            if let Some(rooted) = target.strip_prefix('\\') {
                // Relative to the root of the link's drive.
                if rooted.starts_with('\\') {
                    return Err(Elsewhere(format!("leads to '{target}'; not followed")));
                }
                (link_drive, rooted.to_owned())
            } else {
                let parent = link_tail.rsplit_once('\\').map_or("", |(p, _)| p);
                (link_drive, format!("{parent}\\{target}"))
            }
        }
    };
    // Lexical `.` and `..`, as the system resolves a link's target.
    let mut comps: Vec<&str> = Vec::new();
    for c in tail.split(['\\', '/']).chain(rest.iter().map(String::as_str)) {
        match c {
            "" | "." => {}
            ".." => {
                comps
                    .pop()
                    .ok_or_else(|| Malformed(format!("'{link}' leads above the root of its drive")))?;
            }
            c => comps.push(c),
        }
    }
    Ok(format!(r"\\?\{}:\{}", drive.to_ascii_uppercase(), comps.join("\\")))
}

/// What the walk reads the tree through. `list` is called for the
/// root and for every directory about to be treated as a container;
/// an `Err` makes that directory skipped, with the error as reason.
pub trait Tree {
    fn list(&mut self, rel: &[String]) -> Result<Vec<Entry>, String>;
    fn resolve_link(&mut self, rel: &[String]) -> LinkTarget;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Directory: non-inheriting traverse + list + read attributes.
    Container,
    /// Directory: inheriting read + execute; not descended.
    Granted,
    /// File: read + execute on the file itself.
    File,
    /// Protected exclusion: read deny (inheriting on a directory).
    Denied,
    /// An ungranted-only exclusion, or a directory inside one on the
    /// way to a protected path: nothing written, nothing granted.
    Ungranted,
    /// Nothing written.
    Skipped,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Container => "container",
            Kind::Granted => "granted",
            Kind::File => "file",
            Kind::Denied => "denied",
            Kind::Ungranted => "ungranted",
            Kind::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "container" => Kind::Container,
            "granted" => Kind::Granted,
            "file" => Kind::File,
            "denied" => Kind::Denied,
            "ungranted" => Kind::Ungranted,
            "skipped" => Kind::Skipped,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Components beneath the profile; empty for the root.
    pub rel: Vec<String>,
    pub is_dir: bool,
    pub kind: Kind,
    /// Not on disk: an excluded directory, or a directory on the way
    /// to one, that is to be created empty.
    pub missing: bool,
    /// A denied directory whose deny another part of this crate
    /// writes: recorded as excluded, not written.
    pub stamp: bool,
    /// Why a skipped node was skipped.
    pub note: Option<String>,
}

/// A link the walk met where a rule points: its path, and the rules
/// that matched down to and including its name.
struct LinkHit {
    rel: Vec<String>,
    rules: Vec<usize>,
}

/// What a walk produces.
struct Found {
    nodes: Vec<Node>,
    hits: Vec<LinkHit>,
}

/// How many times the rules may grow by resolving links: the length of a
/// chain of links that each sit beneath the previous one's target.
const MAX_LINK_ROUNDS: usize = 8;

/// The plan for a profile: every node the grant touches or reports,
/// parents before children, in the order the tree listed them.
pub fn plan(tree: &mut dyn Tree, rules: &Rules) -> Result<Vec<Node>, String> {
    plan_resolved(tree, rules).map(|(nodes, _)| nodes)
}

/// [`plan`], with the rules it ended up with: those carried over to the
/// targets of links included, which are what is recorded.
pub fn plan_resolved(tree: &mut dyn Tree, rules: &Rules) -> Result<(Vec<Node>, Rules), String> {
    let mut rules = rules.clone();
    for _ in 0..=MAX_LINK_ROUNDS {
        let entries = tree.list(&[])?;
        let mut found = Found {
            nodes: vec![Node {
                rel: Vec::new(),
                is_dir: true,
                kind: Kind::Container,
                missing: false,
                stamp: true,
                note: None,
            }],
            hits: Vec::new(),
        };
        let all = rules.all();
        walk(tree, &rules, &[], entries, &all, false, &mut found);
        let Found { nodes: out, hits } = found;
        let mut grew = false;
        for hit in hits {
            let LinkTarget::Inside(target) = tree.resolve_link(&hit.rel) else {
                continue;
            };
            if target.is_empty() {
                return Err(format!(
                    "'{}' is a link to the profile directory itself and stands where an \
                     exclusion applies; the profile cannot be granted around it",
                    hit.rel.join("\\")
                ));
            }
            let depth = hit.rel.len();
            for i in hit.rules {
                let from = rules.rules[i].clone();
                let mut pats: Vec<Pat> = target.iter().map(|c| Pat::Lit(norm_component(c))).collect();
                pats.extend(from.pats[depth..].iter().cloned());
                let mut spelled = target.clone();
                spelled.extend(from.spelled[depth..].iter().cloned());
                let same = |r: &Rule| r.pats == pats && r.ungranted == from.ungranted;
                if !rules.rules.iter().any(same) {
                    // The carried-over rule is the source rule at another place,
                    // with its `stamp` and `placeholder` as the source has them.
                    rules.rules.push(Rule {
                        pats,
                        spelled,
                        stamp: from.stamp,
                        ungranted: from.ungranted,
                        placeholder: from.placeholder,
                    });
                    grew = true;
                }
            }
        }
        if !grew {
            return Ok((out, rules));
        }
    }
    Err(format!(
        "links beneath the profile lead on to further links more than {MAX_LINK_ROUNDS} times"
    ))
}

/// Walk one container. `alive` are the rules whose first `rel.len()`
/// components matched the path so far.
fn walk(
    tree: &mut dyn Tree,
    rules: &Rules,
    rel: &[String],
    entries: Vec<Entry>,
    alive: &[usize],
    bare: bool,
    found: &mut Found,
) {
    let depth = rel.len();
    let node = |name: &str, is_dir: bool, kind: Kind| Node {
        rel: rel.iter().cloned().chain([name.to_owned()]).collect(),
        is_dir,
        kind,
        missing: false,
        stamp: true,
        note: None,
    };
    for e in &entries {
        let name = e.name.long.as_str();
        let next: Vec<usize> = alive
            .iter()
            .copied()
            .filter(|&i| e.name.matched_by(&rules.rules[i].pats[depth]))
            .collect();
        if let Some(why) = &e.skip {
            found.nodes.push(Node {
                note: Some(why.clone()),
                ..node(name, e.is_dir, Kind::Skipped)
            });
            // Nothing is written on it. If rules point at or through
            // it and it leads somewhere, they follow it there.
            if e.link && !next.is_empty() {
                found.hits.push(LinkHit {
                    rel: rel.iter().cloned().chain([name.to_owned()]).collect(),
                    rules: next,
                });
            }
            continue;
        }
        let ending: Vec<&Rule> = next
            .iter()
            .map(|&i| &rules.rules[i])
            .filter(|r| r.pats.len() == depth + 1)
            .collect();
        // A protected rule ending here wins over an ungranted-only one.
        let protected: Vec<&&Rule> = ending.iter().filter(|r| !r.ungranted).collect();
        if !protected.is_empty() {
            found.nodes.push(Node {
                stamp: protected.iter().all(|r| r.stamp),
                ..node(name, e.is_dir, Kind::Denied)
            });
            continue;
        }
        if bare || !ending.is_empty() {
            // An ungranted-only exclusion, or something inside one:
            // no entry of any kind.
            if !e.is_dir {
                continue;
            }
            let deeper: Vec<usize> = next
                .iter()
                .copied()
                .filter(|&i| rules.rules[i].pats.len() > depth + 1)
                .collect();
            if !deeper.is_empty() {
                let child: Vec<String> = rel.iter().cloned().chain([name.to_owned()]).collect();
                match tree.list(&child) {
                    Ok(children) => {
                        found.nodes.push(node(name, true, Kind::Ungranted));
                        walk(tree, rules, &child, children, &deeper, true, found);
                    }
                    Err(why) => found.nodes.push(Node {
                        note: Some(why),
                        ..node(name, true, Kind::Skipped)
                    }),
                }
            } else if !ending.is_empty() {
                found.nodes.push(node(name, true, Kind::Ungranted));
            }
            continue;
        }
        if !e.is_dir {
            let readable = if depth == 0 {
                let n = norm_component(name);
                READABLE_ROOT_FILES.iter().any(|f| norm_component(f) == n)
            } else {
                true
            };
            if readable {
                found.nodes.push(node(name, false, Kind::File));
            }
            continue;
        }
        if next.is_empty() {
            found.nodes.push(node(name, true, Kind::Granted));
            continue;
        }
        let child: Vec<String> = rel.iter().cloned().chain([name.to_owned()]).collect();
        match tree.list(&child) {
            Ok(children) => {
                found.nodes.push(node(name, true, Kind::Container));
                walk(tree, rules, &child, children, &next, false, found);
            }
            Err(why) => found.nodes.push(Node {
                note: Some(why),
                ..node(name, true, Kind::Skipped)
            }),
        }
    }
    // Missing excluded directories are created empty so the deny has an
    // object to sit on; never inside or through an ungranted-only exclusion.
    if bare {
        return;
    }
    let mut made: Vec<String> = Vec::new();
    for &i in alive {
        let r = &rules.rules[i];
        if !r.placeholder {
            continue;
        }
        if entries.iter().any(|e| e.name.matched_by(&r.pats[depth])) {
            continue;
        }
        // The other rules that start with the same missing name share
        // the directories they have in common.
        for end in depth + 1..=r.pats.len() {
            let chain: Vec<String> = rel
                .iter()
                .cloned()
                .chain(r.spelled[depth..end].iter().cloned())
                .collect();
            let key = chain[depth..]
                .iter()
                .map(|c| norm_component(c))
                .collect::<Vec<_>>()
                .join("\\");
            if made.contains(&key) {
                continue;
            }
            made.push(key);
            let last = end == r.pats.len();
            // A shorter rule may end at this directory: then it is the
            // excluded one, and nothing is created beneath it.
            let names: Vec<Name> = chain.iter().map(|c| Name::long(c)).collect();
            if rules.ungranted(&names) {
                break;
            }
            let excluded_here = rules.excludes(&names);
            found.nodes.push(Node {
                rel: chain,
                is_dir: true,
                kind: if last || excluded_here {
                    Kind::Denied
                } else {
                    Kind::Container
                },
                missing: true,
                stamp: true,
                note: None,
            });
            if excluded_here && !last {
                break;
            }
        }
    }
}

// ─── A session's read deny on top of the grant ─────────────────────────

/// The recorded paths of a profile read grant that carry an allow of
/// their own - containers, granted directories, granted files - held
/// in the form they are compared in.
#[derive(Debug, Clone, Default)]
pub struct RecordedAllows {
    /// `(as recorded, normalized components)`.
    paths: Vec<(String, Vec<String>)>,
}

fn norm_path(p: &str) -> Vec<String> {
    split_path(p).iter().map(|c| norm_component(c)).collect()
}

impl RecordedAllows {
    pub fn new<'a>(recorded: impl IntoIterator<Item = (&'a str, Kind)>) -> Self {
        Self {
            paths: recorded
                .into_iter()
                .filter(|(_, kind)| matches!(kind, Kind::Container | Kind::Granted | Kind::File))
                .map(|(path, _)| (path.to_owned(), norm_path(path)))
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// The recorded path that `path` itself is, if it is one.
    pub fn at(&self, path: &str) -> Option<&str> {
        let path = norm_path(path);
        self.paths
            .iter()
            .find(|(_, p)| *p == path)
            .map(|(recorded, _)| recorded.as_str())
    }

    /// The recorded paths a read deny on `deny` has to be written on
    /// as well, for it to hold against the grant.
    pub fn covered(&self, deny: &str, allows: &[String]) -> Vec<&str> {
        let deny = norm_path(deny);
        let beneath = |path: &[String], dir: &[String]| path.len() >= dir.len() && path[..dir.len()] == dir[..];
        let spared: Vec<Vec<String>> = allows
            .iter()
            .map(|a| norm_path(a))
            .filter(|a| a.len() > deny.len() && beneath(a, &deny))
            .collect();
        self.paths
            .iter()
            .filter(|(_, path)| path.len() > deny.len() && beneath(path, &deny))
            .filter(|(_, path)| !spared.iter().any(|a| beneath(path, a)))
            .map(|(recorded, _)| recorded.as_str())
            .collect()
    }
}

/// [`RecordedAllows::covered`] for one deny and no session allows.
#[cfg(test)]
fn covered_by_read_deny<'a>(deny: &str, recorded: impl IntoIterator<Item = (&'a str, Kind)>) -> Vec<&'a str> {
    let all: Vec<(&'a str, Kind)> = recorded.into_iter().collect();
    let index = RecordedAllows::new(all.iter().copied());
    let covered: Vec<String> = index.covered(deny, &[]).into_iter().map(str::to_owned).collect();
    all.into_iter()
        .map(|(p, _)| p)
        .filter(|p| covered.iter().any(|c| c == p))
        .collect()
}

// ─── An exclusion whose deny cannot be written ─────────────────────────

/// One entry of an object's access list, as far as it decides reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AceFact<'a> {
    pub deny: bool,
    pub sid: &'a str,
    pub mask: u32,
    /// The entry is a template for objects beneath and does not apply
    /// to this object.
    pub inherit_only: bool,
    /// Objects beneath inherit the entry (to files, to directories,
    /// or both).
    pub inherited_beneath: bool,
    /// The entry is itself a copy inherited from a directory above.
    pub inherited: bool,
}

/// Groups every token of the sandbox account carries, besides its own
/// SID and the sandbox group's: `Everyone`, `Authenticated Users`,
/// `BUILTIN\Users` (the account is made a member).
pub const SANDBOX_ALWAYS: [&str; 3] = ["S-1-1-0", "S-1-5-11", "S-1-5-32-545"];

/// Groups its token carries depending on how it was logged on.
pub const SANDBOX_MAYBE: [&str; 9] = [
    "S-1-5-4",
    "S-1-5-3",
    "S-1-5-6",
    "S-1-5-2",
    "S-1-2-0",
    "S-1-2-1",
    "S-1-5-15",
    "S-1-5-113",
    "S-1-5-64-10",
];

/// Read data / list directory, and the generic rights that include it.
const READS: u32 = 0x1 | 0x8000_0000 | 0x1000_0000;

/// Whether an object's own access list may let the sandbox account read
/// it. No access list at all admits everyone; a group the account only may
/// be in counts for an allow, never for a deny.
pub fn may_read(dacl: Option<&[AceFact<'_>]>, certain: &[&str], maybe: &[&str]) -> bool {
    let Some(dacl) = dacl else {
        return true;
    };
    reading_entry(dacl, certain, maybe).is_some()
}

/// The entry of `dacl` that lets the sandbox account read - on the
/// object, or failing that in what it hands down ([`may_read`]).
/// `None` if nothing does.
pub fn reading_entry<'a>(dacl: &'a [AceFact<'a>], certain: &[&str], maybe: &[&str]) -> Option<&'a AceFact<'a>> {
    let here = dacl.iter().filter(|a| !a.inherit_only);
    let beneath = dacl.iter().filter(|a| a.inherited_beneath);
    first_word_on_reading(here, certain, maybe).or_else(|| first_word_on_reading(beneath, certain, maybe))
}

/// The first entry among `aces` that speaks of reading and names the
/// account decides, as an access check does.
fn first_word_on_reading<'a>(
    aces: impl Iterator<Item = &'a AceFact<'a>>,
    certain: &[&str],
    maybe: &[&str],
) -> Option<&'a AceFact<'a>> {
    for ace in aces.filter(|a| a.mask & READS != 0) {
        let is_certain = certain.contains(&ace.sid);
        if ace.deny && is_certain {
            return None;
        }
        if !ace.deny && (is_certain || maybe.contains(&ace.sid)) {
            return Some(ace);
        }
    }
    None
}

/// What an excluded path is when its deny could not be written. The first
/// layer holds regardless: nothing above it carries an inheriting allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unstamped {
    /// Its own access list gives the sandbox account no read - neither
    /// on the object nor, for a directory, in what it hands down to
    /// the objects beneath: closed without the deny.
    SelfClosed,
    /// Its own access list lets the sandbox account read it, and that
    /// cannot be changed from here. Reported, loudly; the grant is not
    /// what opened it and cannot close it.
    Unprotected,
    /// The access list cannot even be read by the calling user: not
    /// its owner, and given no right to. Nobody established that the
    /// sandbox account is kept out.
    Unverifiable,
}

/// `dacl`: `Err` if the access list could not be read.
pub fn unstamped_exclusion(dacl: Result<Option<&[AceFact<'_>]>, ()>, certain: &[&str], maybe: &[&str]) -> Unstamped {
    match dacl {
        Err(()) => Unstamped::Unverifiable,
        Ok(dacl) if may_read(dacl, certain, maybe) => Unstamped::Unprotected,
        Ok(_) => Unstamped::SelfClosed,
    }
}

// ─── Stores an environment variable moves ───────────────────────────

/// What a variable that relocates a protected store names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moved {
    /// A directory; these paths in it are the protected ones.
    Beneath(&'static [&'static str]),
    /// The protected file or directory itself.
    Itself,
    /// A `;`-separated list of protected files.
    List,
}

/// Variables that move a protected store. What one names, when inside the
/// profile, is protected as well as the store's default place.
pub const ENV_STORES: &[(&str, Moved)] = &[
    ("CARGO_HOME", Moved::Beneath(&["credentials", "credentials.toml"])),
    (
        "GRADLE_USER_HOME",
        Moved::Beneath(&[r"enterprise\keys.properties", r"develocity\keys.properties"]),
    ),
    ("GNUPGHOME", Moved::Itself),
    ("DOCKER_CONFIG", Moved::Itself),
    ("AZURE_CONFIG_DIR", Moved::Itself),
    ("GH_CONFIG_DIR", Moved::Itself),
    ("AWS_SHARED_CREDENTIALS_FILE", Moved::Itself),
    ("AWS_CONFIG_FILE", Moved::Itself),
    ("NETRC", Moved::Itself),
    ("KUBECONFIG", Moved::List),
];

/// A path as a variable gives it: separators one way, none at the end.
fn env_path(v: &str) -> Option<String> {
    let v = v.trim().replace('/', "\\");
    let v = v.trim_end_matches('\\');
    (!v.is_empty()).then(|| v.to_owned())
}

/// Whether `p` names a place whatever the current directory is: a
/// drive path (`X:\…`) or a UNC or device path (`\\…`).
fn is_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with(r"\\") || (b.len() >= 3 && b[0].is_ascii_alphabetic() && &b[1..3] == b":\\")
}

/// The absolute paths the calling user's environment (`own_env`) says
/// hold a protected store ([`ENV_STORES`]), and the relative values it
/// ignored as `VAR=value`: a relative path names nothing fixed.
pub fn env_exclusions(own_env: &dyn Fn(&str) -> Option<String>) -> (Vec<String>, Vec<String>) {
    let (mut out, mut ignored) = (Vec::new(), Vec::new());
    for (var, moved) in ENV_STORES {
        let Some(value) = own_env(var) else {
            continue;
        };
        let values: Vec<String> = match moved {
            Moved::List => value.split(';').filter_map(env_path).collect(),
            _ => env_path(&value).into_iter().collect(),
        };
        for v in values {
            if !is_absolute(&v) {
                ignored.push(format!("{var}={v}"));
            } else if let Moved::Beneath(rel) = moved {
                out.extend(rel.iter().map(|r| format!("{v}\\{r}")));
            } else {
                out.push(v);
            }
        }
    }
    (out, ignored)
}

/// Whether both credential files of the cargo home `dir` are among the
/// recorded protected exclusions. Only then is `dir` offered as
/// `CARGO_HOME`.
pub fn cargo_home_protected(exclusions: &[String], dir: &str) -> bool {
    let key = |p: &str| {
        p.strip_prefix(r"\\?\")
            .unwrap_or(p)
            .trim_end_matches('\\')
            .to_lowercase()
    };
    ["credentials", "credentials.toml"].iter().all(|f| {
        let want = format!("{}\\{f}", key(dir));
        exclusions.iter().any(|e| key(e) == want)
    })
}

// ─── Configuration pointers ─────────────────────────────────────────

/// Variables an embedder may set so a tool reads the real user's files in
/// place, each only when its file or directory is there and covered by the
/// grant. `RUSTUP_HOME` and `CARGO_HOME` name directories.
pub fn env_pointers(
    profile: &str,
    roaming: Option<&str>,
    readable: &dyn Fn(&str) -> bool,
    dir_readable: &dyn Fn(&str) -> bool,
    own_env: &dyn Fn(&str) -> Option<String>,
) -> Vec<(&'static str, String)> {
    let at = |base: &str, rel: &str| format!("{}\\{rel}", base.trim_end_matches('\\'));
    let first = |candidates: &[String]| candidates.iter().find(|p| readable(p)).cloned();
    let mut out = Vec::new();
    if let Some(p) = first(&[at(profile, ".npmrc")]) {
        out.push(("NPM_CONFIG_USERCONFIG", p));
    }
    if let Some(p) = first(&[at(profile, ".gitconfig"), at(profile, r".config\git\config")]) {
        out.push(("GIT_CONFIG_GLOBAL", p));
    }
    let mut pip = Vec::new();
    pip.extend(roaming.map(|r| at(r, r"pip\pip.ini")));
    pip.push(at(profile, r"pip\pip.ini"));
    pip.push(at(profile, r".config\pip\pip.conf"));
    if let Some(p) = first(&pip) {
        out.push(("PIP_CONFIG_FILE", p));
    }
    if let Some(settings) = first(&[at(profile, r".m2\settings.xml")]) {
        let mut v = format!("-s \"{settings}\"");
        if let Some(sec) = first(&[at(profile, r".m2\settings-security.xml")]) {
            v.push_str(&format!(" \"-Dsettings.security={sec}\""));
        }
        out.push(("MAVEN_ARGS", v));
    }
    for (var, rel) in [("RUSTUP_HOME", ".rustup"), ("CARGO_HOME", ".cargo")] {
        let dir = own_env(var)
            .map(|v| v.trim_end_matches('\\').to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| at(profile, rel));
        if dir_readable(&dir) {
            out.push((var, dir));
        }
    }
    out
}

#[cfg(test)]
mod tests;
