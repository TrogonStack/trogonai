//! From a filesystem policy in guest paths to Seatbelt filters on host paths.
//!
//! Seatbelt has no mount namespace, so a path the policy names under the
//! workspace (`/workspace/...`) is rewritten to where the workspace really
//! is, and every path is emitted under both spellings the kernel may report
//! (`/tmp` and `/private/tmp`). A path that cannot be rewritten or expressed
//! fails the launch: a rule that silently matched nothing would fail open.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, GlobPattern, LiteralPath};
use trogon_sandbox_protocol::{GlobMatcher, GlobToken, LiteralPathExt};

use super::sbpl::{Filter, SbplError, escape_regex};
use crate::isolation::mandatory::{
    DANGEROUS_DIRECTORIES, DANGEROUS_FILES, DOT_AGENT, DOT_AGENT_PROTECTED, DOT_GIT, GIT_HOOKS, git_dir_protected,
};

#[derive(Debug, Clone)]
pub struct PathMapper {
    mounts: Vec<(LiteralPath, LiteralPath)>,
}

impl PathMapper {
    pub fn new(mut mounts: Vec<(LiteralPath, LiteralPath)>) -> Self {
        mounts.sort_by_key(|(guest, _)| std::cmp::Reverse(guest.as_str().len()));
        Self { mounts }
    }

    pub fn is_identity(&self) -> bool {
        self.mounts.is_empty()
    }

    fn map(&self, guest: &LiteralPath) -> anyhow::Result<LiteralPath> {
        for (mount_guest, host) in &self.mounts {
            if guest.is_within(mount_guest) {
                let rest = if mount_guest.is_root() {
                    guest.as_str()
                } else {
                    &guest.as_str()[mount_guest.as_str().len()..]
                };
                let joined = format!("{}{rest}", host.as_str().trim_end_matches('/'));
                let joined = if joined.is_empty() { "/".to_string() } else { joined };
                return LiteralPath::new(&joined).with_context(|| format!("mapping {guest} onto {host}"));
            }
        }
        Ok(guest.clone())
    }

    /// The host paths a guest path may be seen as: where it maps to, and
    /// that location with its symlinks resolved when that differs.
    pub fn host_spellings(&self, guest: &LiteralPath) -> anyhow::Result<Vec<LiteralPath>> {
        let mapped = self.map(guest)?;
        let resolved = resolve(&mapped);
        Ok(spellings_of(mapped, resolved))
    }

    /// The spellings of a path that is a name to allow: the links on the way
    /// are resolved, the last component stays as written. A link planted at
    /// the name then allows nothing, instead of widening the grant to
    /// wherever it points.
    pub fn name_spellings(&self, guest: &LiteralPath) -> anyhow::Result<Vec<LiteralPath>> {
        let mapped = self.map(guest)?;
        let located = match (mapped.parent(), mapped.components().last()) {
            (Some(parent), Some(name)) => {
                let parent = resolve(&parent);
                let raw = if parent.is_root() {
                    format!("/{name}")
                } else {
                    format!("{parent}/{name}")
                };
                LiteralPath::new(&raw).unwrap_or_else(|_| mapped.clone())
            }
            _ => mapped.clone(),
        };
        Ok(spellings_of(mapped, located))
    }
}

/// The path as written and, when it adds a second spelling, where the links
/// lead. Neither the root nor a device is ever a second spelling: a rule on a
/// name that was blanked with a link to `/dev/null` would be a rule on
/// `/dev/null` for every command.
fn spellings_of(mapped: LiteralPath, resolved: LiteralPath) -> Vec<LiteralPath> {
    let in_dev = resolved.as_str() == DEVICE_DIRECTORY || resolved.as_str().starts_with(DEVICE_DIRECTORY_PREFIX);
    if resolved == mapped || resolved.is_root() || in_dev {
        return vec![mapped];
    }
    vec![mapped, resolved]
}

fn resolve(path: &LiteralPath) -> LiteralPath {
    let mut current = PathBuf::from(path.as_str());
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&current) {
            let mut rebuilt = real;
            rebuilt.extend(tail.iter().rev());
            return rebuilt
                .to_str()
                .and_then(|s| LiteralPath::new(s).ok())
                .unwrap_or_else(|| path.clone());
        }
        let Some(name) = current.file_name().map(|n| n.to_owned()) else {
            return path.clone();
        };
        tail.push(name);
        if !current.pop() {
            return path.clone();
        }
    }
}

/// Some paths of a policy list as Seatbelt filters, with the literal
/// directories they hang from.
#[derive(Debug, Clone, Default)]
pub struct Region {
    pub filters: Vec<Filter>,
    pub anchors: Vec<LiteralPath>,
    pub glob_bases: Vec<LiteralPath>,
}

impl Region {
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }

    pub fn translate(entries: &[FsPathEntry], mapper: &PathMapper) -> anyhow::Result<Self> {
        Self::translate_with(entries, mapper, PathMapper::host_spellings, true)
    }

    /// Like [`Region::translate`] for entries that grant access: a literal
    /// path keeps its last component as written.
    pub fn translate_allows(entries: &[FsPathEntry], mapper: &PathMapper) -> anyhow::Result<Self> {
        Self::translate_with(entries, mapper, PathMapper::name_spellings, false)
    }

    fn translate_with(
        entries: &[FsPathEntry],
        mapper: &PathMapper,
        literal_spellings: fn(&PathMapper, &LiteralPath) -> anyhow::Result<Vec<LiteralPath>>,
        subtree: bool,
    ) -> anyhow::Result<Self> {
        let mut region = Region::default();
        let mut literals: Vec<LiteralPath> = Vec::new();
        for entry in entries {
            match entry {
                FsPathEntry::Literal { path, .. } => {
                    for host in literal_spellings(mapper, path)? {
                        region.anchors.push(host.clone());
                        literals.push(host);
                    }
                }
                FsPathEntry::Glob { pattern } => {
                    let matcher = GlobMatcher::new(pattern);
                    let base = matcher.static_base()?;
                    for host in mapper.host_spellings(&base)? {
                        region
                            .filters
                            .push(glob_filter(pattern, &matcher, &base, &host, subtree)?);
                        region.glob_bases.push(host.clone());
                        region.anchors.push(host);
                    }
                }
            }
        }
        literals.sort();
        literals.dedup();
        region.filters.extend(subtree_filters(&literals)?);
        region.anchors.sort();
        region.anchors.dedup();
        Ok(region)
    }
}

fn glob_filter(
    pattern: &GlobPattern,
    matcher: &GlobMatcher,
    guest_base: &LiteralPath,
    host_base: &LiteralPath,
    subtree: bool,
) -> Result<Filter, SbplError> {
    let skipped = guest_base.components().count();
    let tail = tail_after(matcher.tokens(), skipped)
        .ok_or_else(|| SbplError::UnsupportedGlob(pattern.as_str().to_string()))?;
    let mut source = String::from("^");
    if !host_base.is_root() {
        source.push_str(&escape_regex(host_base.as_str()));
    }
    for token in tail {
        source.push_str(&token_regex(token)?);
    }
    source.push_str(if subtree { SUBTREE_TAIL } else { "$" });
    Filter::regex(&source)
}

fn tail_after(tokens: &[GlobToken], base_segments: usize) -> Option<&[GlobToken]> {
    let mut separators = 0;
    for (index, token) in tokens.iter().enumerate() {
        if *token == GlobToken::Separator {
            if separators == base_segments {
                return Some(&tokens[index..]);
            }
            separators += 1;
        }
    }
    None
}

/// The regex of a whole glob pattern, anchored at both ends.
#[cfg(test)]
pub fn glob_regex(pattern: &GlobPattern) -> Result<String, SbplError> {
    let mut source = String::from("^");
    for token in GlobMatcher::new(pattern).tokens() {
        source.push_str(&token_regex(token)?);
    }
    source.push('$');
    Ok(source)
}

fn token_regex(token: &GlobToken) -> Result<String, SbplError> {
    Ok(match token {
        GlobToken::Text(text) => escape_regex(text),
        GlobToken::Separator => "/".to_string(),
        GlobToken::AnyChar => "[^/]".to_string(),
        GlobToken::Star => "[^/]*".to_string(),
        GlobToken::Globstar => ".*".to_string(),
        GlobToken::GlobstarSlash => "(.*/)?".to_string(),
        GlobToken::Set(ranges) => set_regex(ranges)?,
    })
}

fn unit_char(unit: u16) -> char {
    char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)
}

fn set_regex(ranges: &[(u16, u16)]) -> Result<String, SbplError> {
    let mut alternatives: Vec<String> = Vec::new();
    for &(from_unit, to_unit) in ranges {
        let (from, to) = (unit_char(from_unit), unit_char(to_unit));
        if from_unit == to_unit {
            alternatives.push(escape_regex(&from.to_string()));
        } else if from.is_ascii_alphanumeric()
            && to.is_ascii_alphanumeric()
            && from.is_ascii_digit() == to.is_ascii_digit()
            && from.is_ascii_uppercase() == to.is_ascii_uppercase()
        {
            alternatives.push(format!("[{from}-{to}]"));
        } else if u32::from(to_unit) - u32::from(from_unit) < MAX_SET_SPAN {
            for unit in from_unit..=to_unit {
                alternatives.push(escape_regex(&unit_char(unit).to_string()));
            }
        } else {
            return Err(SbplError::SetTooWide { from, to });
        }
    }
    Ok(format!("({})", alternatives.join("|")))
}

/// Subtree filters for literal paths. Four or more that share a directory
/// become regexes over their names, split so each stays under
/// [`MAX_REGEX_BYTES`]; the rest are plain subpaths. Paths shaped
/// `<dir>/<name>/<leaf>` that share `dir` and the same leaves fold into one
/// regex over the names first.
pub fn subtree_filters(paths: &[LiteralPath]) -> Result<Vec<Filter>, SbplError> {
    let (mut filters, rest) = leaf_groups(paths)?;
    let mut by_parent: BTreeMap<LiteralPath, Vec<&str>> = BTreeMap::new();
    for path in &rest {
        match (path.parent(), path.components().last()) {
            (Some(parent), Some(name)) => by_parent.entry(parent).or_default().push(name),
            _ => filters.push(Filter::subpath(path.as_str())?),
        }
    }
    for (parent, names) in by_parent {
        let prefix = format!("^{}/", escape_regex(parent.as_str().trim_end_matches('/')));
        let (grouped, long) = split_by_fit(&prefix, SUBTREE_TAIL, names);
        for name in long {
            filters.push(Filter::subpath(&join(&parent, name))?);
        }
        for source in alternation_regexes(&prefix, &grouped, SUBTREE_TAIL)? {
            filters.push(Filter::regex(&source)?);
        }
    }
    Ok(filters)
}

/// Literal filters for exactly these paths. Four or more that share a
/// directory become regexes over their names; the rest are plain literals.
pub fn exact_filters(paths: &[LiteralPath]) -> Result<Vec<Filter>, SbplError> {
    let mut filters = Vec::new();
    let mut by_parent: BTreeMap<LiteralPath, Vec<&str>> = BTreeMap::new();
    for path in paths {
        match (path.parent(), path.components().last()) {
            (Some(parent), Some(name)) => by_parent.entry(parent).or_default().push(name),
            _ => filters.push(Filter::literal(path.as_str())?),
        }
    }
    for (parent, names) in by_parent {
        let prefix = format!("^{}/", escape_regex(parent.as_str().trim_end_matches('/')));
        let (grouped, long) = split_by_fit(&prefix, "$", names);
        for name in long {
            filters.push(Filter::literal(&join(&parent, name))?);
        }
        for source in alternation_regexes(&prefix, &grouped, "$")? {
            filters.push(Filter::regex(&source)?);
        }
    }
    Ok(filters)
}

/// The escaped names that go into a grouped regex and the names that stay
/// plain paths: names too long for one regex, or all of them when fewer
/// than [`MIN_GROUP_SIZE`] fit.
fn split_by_fit<'a>(prefix: &str, suffix: &str, names: Vec<&'a str>) -> (Vec<String>, Vec<&'a str>) {
    let overhead = prefix.len() + suffix.len() + 2;
    let (fit, long): (Vec<&str>, Vec<&str>) = names
        .into_iter()
        .partition(|name| overhead + escape_regex(name).len() <= MAX_REGEX_BYTES);
    if fit.len() < MIN_GROUP_SIZE {
        return (Vec::new(), fit.into_iter().chain(long).collect());
    }
    (fit.into_iter().map(escape_regex).collect(), long)
}

fn join(parent: &LiteralPath, name: &str) -> String {
    if parent.is_root() {
        format!("/{name}")
    } else {
        format!("{}/{name}", parent.as_str())
    }
}

fn leaf_groups(paths: &[LiteralPath]) -> Result<(Vec<Filter>, Vec<LiteralPath>), SbplError> {
    let mut by_dir: BTreeMap<LiteralPath, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for path in paths {
        let (Some(parent), Some(leaf)) = (path.parent(), path.components().last()) else {
            continue;
        };
        let (Some(dir), Some(name)) = (parent.parent(), parent.components().last()) else {
            continue;
        };
        if dir.is_root() {
            continue;
        }
        by_dir
            .entry(dir)
            .or_default()
            .entry(name.to_string())
            .or_default()
            .insert(leaf.to_string());
    }
    let mut grouped: BTreeSet<LiteralPath> = BTreeSet::new();
    let mut filters = Vec::new();
    for (dir, names) in by_dir {
        let mut by_signature: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
        for (name, leaves) in names {
            by_signature.entry(leaves.into_iter().collect()).or_default().push(name);
        }
        for (leaves, names) in by_signature {
            let prefix = format!("^{}/", escape_regex(dir.as_str()));
            let suffix = format!(
                "/({}){SUBTREE_TAIL}",
                leaves
                    .iter()
                    .map(|leaf| escape_regex(leaf))
                    .collect::<Vec<_>>()
                    .join("|")
            );
            let (fragments, _) = split_by_fit(&prefix, &suffix, names.iter().map(String::as_str).collect());
            if fragments.is_empty() {
                continue;
            }
            let names: Vec<String> = names
                .into_iter()
                .filter(|name| fragments.contains(&escape_regex(name)))
                .collect();
            for source in alternation_regexes(&prefix, &fragments, &suffix)? {
                filters.push(Filter::regex(&source)?);
            }
            for name in &names {
                for leaf in &leaves {
                    if let Ok(path) = LiteralPath::new(format!("{}/{name}/{leaf}", dir.as_str())) {
                        grouped.insert(path);
                    }
                }
            }
        }
    }
    let rest = paths.iter().filter(|path| !grouped.contains(*path)).cloned().collect();
    Ok((filters, rest))
}

/// `prefix(a|b|...)suffix`, split into as many regexes as the budget needs.
pub fn alternation_regexes(prefix: &str, fragments: &[String], suffix: &str) -> Result<Vec<String>, SbplError> {
    let overhead = prefix.len() + suffix.len() + 2;
    let mut regexes = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut length = overhead;
    let render = |current: &[&str]| format!("{prefix}({}){suffix}", current.join("|"));
    for fragment in fragments {
        if overhead + fragment.len() > MAX_REGEX_BYTES {
            return Err(SbplError::StringTooLong {
                len: overhead + fragment.len(),
                max: MAX_REGEX_BYTES,
            });
        }
        let added = fragment.len() + usize::from(!current.is_empty());
        if length + added > MAX_REGEX_BYTES {
            regexes.push(render(&current));
            current.clear();
            length = overhead;
        }
        length += fragment.len() + usize::from(!current.is_empty());
        current.push(fragment);
    }
    if !current.is_empty() {
        regexes.push(render(&current));
    }
    Ok(regexes)
}

fn fold_case(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphabetic() {
                format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
            } else {
                escape_regex(&c.to_string())
            }
        })
        .collect()
}

fn folded_alternatives(names: &[&str]) -> String {
    names.iter().map(|name| fold_case(name)).collect::<Vec<_>>().join("|")
}

/// What stays read-only beneath a writable root whatever the policy says,
/// wherever it appears below the root and whether or not it exists yet.
///
/// A protected directory is refused itself only at the top of the root. Below
/// it the rule covers what lies inside, so the directory can be made and its
/// contents cannot be written.
pub fn mandatory_write_denies(root: &LiteralPath, allow_git_config: bool) -> Result<Vec<Filter>, SbplError> {
    let root = escape_regex(root.as_str().trim_end_matches('/'));
    let top = format!("^{root}/");
    let anywhere = format!("^{root}/(.*/)?");
    let mut files: Vec<String> = DANGEROUS_FILES.iter().map(|name| fold_case(name)).collect();
    let git_files: Vec<&str> = git_dir_protected(allow_git_config)
        .iter()
        .copied()
        .filter(|name| *name != GIT_HOOKS)
        .collect();
    files.push(format!("{}/({})", fold_case(DOT_GIT), folded_alternatives(&git_files)));
    let mut directories: Vec<String> = DANGEROUS_DIRECTORIES.iter().map(|name| fold_case(name)).collect();
    directories.push(format!(
        "{}/({})",
        fold_case(DOT_AGENT),
        folded_alternatives(&DOT_AGENT_PROTECTED)
    ));
    directories.push(format!("{}/{}", fold_case(DOT_GIT), fold_case(GIT_HOOKS)));
    let mut sources = alternation_regexes(&anywhere, &files, SUBTREE_TAIL)?;
    sources.extend(alternation_regexes(&top, &directories, SUBTREE_TAIL)?);
    sources.extend(alternation_regexes(&anywhere, &directories, BENEATH_TAIL)?);
    sources.iter().map(|source| Filter::regex(source)).collect()
}

/// The mandatory protections laid out beneath the working directory alone, as
/// write denies: each name at the top of `cwd` and wherever it appears below.
pub fn working_directory_protections(cwd: &LiteralPath, allow_git_config: bool) -> anyhow::Result<Vec<FsPathEntry>> {
    let base = cwd.as_str().trim_end_matches('/');
    let mut entries = Vec::new();
    let mut protect = |relative: String, pattern: String| {
        entries.push(FsPathEntry::Literal {
            path: LiteralPath::new(format!("{base}/{relative}"))?,
            directory: DirectoryMarker::Unmarked,
        });
        entries.push(FsPathEntry::Glob {
            pattern: GlobPattern::anchored(cwd.clone(), pattern)?,
        });
        Ok::<_, anyhow::Error>(())
    };
    for name in DANGEROUS_FILES {
        protect(name.to_string(), format!("/**/{name}"))?;
    }
    let directories = DANGEROUS_DIRECTORIES
        .iter()
        .map(|name| name.to_string())
        .chain(DOT_AGENT_PROTECTED.iter().map(|name| format!("{DOT_AGENT}/{name}")));
    for directory in directories {
        protect(directory.clone(), format!("/**/{directory}/**"))?;
    }
    let hooks = format!("{DOT_GIT}/{GIT_HOOKS}");
    protect(hooks.clone(), format!("/**/{hooks}/**"))?;
    if !allow_git_config {
        let config = format!("{DOT_GIT}/config");
        protect(config.clone(), format!("/**/{config}"))?;
    }
    Ok(entries)
}

/// The directories whose removal or rename would carry a mandatory path away.
pub fn mandatory_move_blocks(root: &LiteralPath) -> Result<Filter, SbplError> {
    let source = format!(
        "^{}/(.*/)?({}|{})$",
        escape_regex(root.as_str().trim_end_matches('/')),
        fold_case(DOT_GIT),
        fold_case(DOT_AGENT)
    );
    Filter::regex(&source)
}

/// The protected directory names at the top of a writable root. Creating
/// one, as a directory or as a link to somewhere else, is refused while
/// the name is free, so a link cannot stand in for a protected directory
/// that does not exist yet.
pub fn mandatory_create_blocks(root: &LiteralPath) -> Result<Filter, SbplError> {
    let source = format!(
        "^{}/({}|{})$",
        escape_regex(root.as_str().trim_end_matches('/')),
        fold_case(DOT_GIT),
        fold_case(DOT_AGENT)
    );
    Filter::regex(&source)
}

/// Directories of glob bases that sit strictly below a writable root. A
/// match of the glob lives in one of them, so moving one away carries the
/// match out of its rule.
pub fn pinned_bases(bases: &[LiteralPath], write_roots: &[LiteralPath]) -> Vec<LiteralPath> {
    let mut pinned: Vec<LiteralPath> = bases
        .iter()
        .filter(|base| write_roots.iter().any(|root| *base != root && base.is_within(root)))
        .cloned()
        .collect();
    pinned.sort();
    pinned.dedup();
    pinned
}

/// Directories strictly between a writable root and a protected path. Moving
/// one of them away would carry the protected path out of its rule.
pub fn pinned_ancestors(protected: &[LiteralPath], write_roots: &[LiteralPath]) -> Vec<LiteralPath> {
    let mut pinned: Vec<LiteralPath> = Vec::new();
    for path in protected {
        for ancestor in path.ancestors().skip(1) {
            let below_a_root = write_roots
                .iter()
                .any(|root| ancestor != *root && ancestor.is_within(root));
            if below_a_root {
                pinned.push(ancestor);
            }
        }
    }
    pinned.sort();
    pinned.dedup();
    pinned
}

pub fn host_path(path: &Path) -> anyhow::Result<LiteralPath> {
    let absolute = std::fs::canonicalize(path).with_context(|| format!("resolving {}", path.display()))?;
    let raw = absolute
        .to_str()
        .with_context(|| format!("{} is not valid UTF-8", absolute.display()))?;
    LiteralPath::new(raw).with_context(|| format!("{raw} is not a usable path"))
}

#[cfg(test)]
mod tests;
use super::constants::{
    BENEATH_TAIL, DEVICE_DIRECTORY, DEVICE_DIRECTORY_PREFIX, MAX_REGEX_BYTES, MAX_SET_SPAN, MIN_GROUP_SIZE,
    SUBTREE_TAIL,
};
