//! The Seatbelt profile of one launch: what the baseline, the filesystem
//! policy and the egress policy add up to.
//!
//! Everything that grants access is emitted first and everything that takes
//! it away after, so a deny is never undone by a rule that comes later. Read
//! denies that must leave some paths readable cannot be expressed as a deny
//! followed by an allow for that reason; the readable paths are carved out of
//! the deny with `require-not` instead.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use anyhow::Context;
use trogon_sandbox_contract::{
    DirectoryMarker, FilesystemOptions, FsPathEntry, FsPolicy, LiteralPath, MacosOptions, ProtectedNamesAnchor,
};
use trogon_sandbox_protocol::{CompiledFsEntry, FsPolicyEvaluator, GlobMatcher, LiteralPathExt};

use super::baseline::baseline_allows;
use super::opt_ins::opt_in_allows;
use super::policy::{
    PathMapper, Region, exact_filters, mandatory_create_blocks, mandatory_move_blocks, mandatory_write_denies,
    pinned_ancestors, pinned_bases, working_directory_protections,
};
use super::sbpl::{AllowRule, DenyRule, Filter, LogTag, Operation, Profile, ProfileBuilder, VnodeType};
use crate::subprocess::EgressPolicy;

#[cfg(test)]
pub fn compose(policy: &FsPolicy, mapper: &PathMapper, egress: &EgressPolicy, tag: LogTag) -> anyhow::Result<Profile> {
    compose_with_opt_ins(
        policy,
        mapper,
        &LiteralPath::new("/workspace")?,
        egress,
        &MacosOptions::default(),
        &FilesystemOptions::default(),
        tag,
    )
}

pub fn compose_with_opt_ins(
    policy: &FsPolicy,
    mapper: &PathMapper,
    cwd: &LiteralPath,
    egress: &EgressPolicy,
    opt_ins: &MacosOptions,
    filesystem: &FilesystemOptions,
    tag: LogTag,
) -> anyhow::Result<Profile> {
    if filesystem.disabled.is_disabled() {
        return unrestricted_filesystem(egress, opt_ins, tag);
    }
    let all_denies: Vec<FsPathEntry> = policy
        .deny_read()
        .iter()
        .chain(policy.credential_deny_read())
        .cloned()
        .collect();
    let deny_read = Region::translate(&all_denies, mapper)?;
    let read_groups = read_groups(policy, mapper)?;
    let allow_write = Region::translate_allows(policy.allow_write(), mapper)?;
    let mut write_denied = policy.deny_write().to_vec();
    let mandatory_roots: &[LiteralPath] = match opt_ins.protected_names_anchor {
        ProtectedNamesAnchor::WriteRoots => &allow_write.anchors,
        ProtectedNamesAnchor::WorkingDirectory => {
            write_denied.extend(working_directory_protections(
                cwd,
                filesystem.allow_git_config.is_writable(),
            )?);
            &[]
        }
    };
    let deny_write = Region::translate(&write_denied, mapper)?;

    let mut builder = ProfileBuilder::new(tag);
    for rule in baseline_allows()? {
        builder = builder.allow(rule);
    }
    builder = builder.allow(AllowRule::new([Operation::FileReadStar], []));
    if !allow_write.is_empty() {
        builder = builder.allow(AllowRule::new([Operation::FileWriteStar], allow_write.filters.clone()));
    }
    for rule in network_allows(egress)? {
        builder = builder.allow(rule);
    }
    for rule in opt_in_allows(opt_ins)? {
        builder = builder.allow(rule);
    }

    let mut builder = builder.begin_denies();
    for rule in write_denies(&deny_write, mandatory_roots, filesystem.allow_git_config.is_writable())? {
        builder = builder.deny(rule);
    }
    for group in &read_groups {
        for rule in read_denies(&group.deny, &group.carve_out)? {
            builder = builder.deny(rule);
        }
    }
    for rule in move_blocks(
        &deny_read,
        &read_groups,
        &deny_write,
        &allow_write,
        mandatory_roots,
        policy.allow_write(),
        mapper,
    )? {
        builder = builder.deny(rule);
    }
    Ok(builder.build()?)
}

fn unrestricted_filesystem(egress: &EgressPolicy, opt_ins: &MacosOptions, tag: LogTag) -> anyhow::Result<Profile> {
    let mut builder = ProfileBuilder::new(tag);
    for rule in baseline_allows()? {
        builder = builder.allow(rule);
    }
    builder = builder.allow(AllowRule::new([Operation::FileReadStar], []));
    builder = builder.allow(AllowRule::new([Operation::FileWriteStar], []));
    for rule in network_allows(egress)? {
        builder = builder.allow(rule);
    }
    for rule in opt_in_allows(opt_ins)? {
        builder = builder.allow(rule);
    }
    Ok(builder.begin_denies().build()?)
}

fn network_allows(egress: &EgressPolicy) -> anyhow::Result<Vec<AllowRule>> {
    Ok(match egress {
        EgressPolicy::DenyAll => vec![],
        EgressPolicy::AllowAll => vec![AllowRule::new([Operation::NetworkStar], [])],
        EgressPolicy::Loopback(addr) => vec![AllowRule::new([Operation::NetworkOutbound], [loopback_filter(addr)?])],
    })
}

fn loopback_filter(addr: &SocketAddr) -> anyhow::Result<Filter> {
    anyhow::ensure!(
        addr.ip().is_loopback(),
        "the egress proxy address {addr} is not on loopback, so the network cannot be limited to it"
    );
    Ok(Filter::remote_ip(&format!("localhost:{}", addr.port()))?)
}

fn write_denies(
    deny_write: &Region,
    mandatory_roots: &[LiteralPath],
    allow_git_config: bool,
) -> anyhow::Result<Vec<DenyRule>> {
    let mut rules = Vec::new();
    if !deny_write.is_empty() {
        rules.push(DenyRule::new([Operation::FileWriteStar], deny_write.filters.clone()));
    }
    for root in mandatory_roots {
        rules.push(DenyRule::new(
            [Operation::FileWriteStar],
            mandatory_write_denies(root, allow_git_config)?,
        ));
    }
    Ok(rules)
}

/// A set of read denies that the same allow entries can re-open.
struct ReadGroup {
    entries: Vec<FsPathEntry>,
    credential: bool,
    deny: Region,
    carve_out: Region,
}

fn read_groups(policy: &FsPolicy, mapper: &PathMapper) -> anyhow::Result<Vec<ReadGroup>> {
    let evaluator = FsPolicyEvaluator::new(policy);
    let own_offset = policy.allow_read_within_deny().len();
    let mut by_exceptions: BTreeMap<(Vec<usize>, bool), Vec<FsPathEntry>> = BTreeMap::new();
    for deny in policy.deny_read() {
        let exceptions = if nested_in_allowed_literal(deny, policy.allow_read_within_deny(), mapper)? {
            Vec::new()
        } else {
            spelled_exceptions(
                deny,
                policy.allow_read_within_deny(),
                evaluator.read_exceptions(deny),
                mapper,
            )?
        };
        by_exceptions.entry((exceptions, false)).or_default().push(deny.clone());
    }
    for deny in policy.credential_deny_read() {
        let own = if nested_in_allowed_literal(deny, policy.own_allow_within_deny(), mapper)? {
            Vec::new()
        } else {
            spelled_exceptions(
                deny,
                policy.own_allow_within_deny(),
                evaluator.credential_read_exceptions(deny),
                mapper,
            )?
            .into_iter()
            .map(|index| own_offset + index)
            .collect()
        };
        by_exceptions.entry((own, true)).or_default().push(deny.clone());
    }
    by_exceptions.retain(|_, denies| !denies.is_empty());
    by_exceptions
        .into_iter()
        .map(|((exceptions, credential), denies)| {
            let allows: Vec<FsPathEntry> = exceptions
                .iter()
                .map(|index| match (*index).checked_sub(own_offset) {
                    Some(own) => policy.own_allow_within_deny()[own].clone(),
                    None => policy.allow_read_within_deny()[*index].clone(),
                })
                .collect();
            Ok(ReadGroup {
                deny: Region::translate(&denies, mapper)?,
                entries: denies,
                credential,
                carve_out: Region::translate_allows(&allows, mapper)?,
            })
        })
        .collect()
}

/// Whether a literal deny sits strictly below a literal allow once the links
/// on both sides are followed, so an allow spelled through a link does not
/// re-open a deny that lies inside the directory it names.
fn nested_in_allowed_literal(deny: &FsPathEntry, allows: &[FsPathEntry], mapper: &PathMapper) -> anyhow::Result<bool> {
    let FsPathEntry::Literal { path: deny, .. } = deny else {
        return Ok(false);
    };
    let denied = mapper.host_spellings(deny)?;
    for allow in allows {
        let FsPathEntry::Literal { path: allow, .. } = allow else {
            continue;
        };
        for dir in mapper.name_spellings(allow)? {
            if denied.iter().any(|path| *path != dir && path.is_within(&dir)) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The allows that can re-open a glob deny, counting an allow that only
/// falls under the glob once the links on its way are followed.
fn spelled_exceptions(
    deny: &FsPathEntry,
    allows: &[FsPathEntry],
    mut exceptions: Vec<usize>,
    mapper: &PathMapper,
) -> anyhow::Result<Vec<usize>> {
    let FsPathEntry::Glob { pattern } = deny else {
        return Ok(exceptions);
    };
    if !mapper.is_identity() {
        return Ok(exceptions);
    }
    let matcher = GlobMatcher::new(pattern);
    for (index, allow) in allows.iter().enumerate() {
        let FsPathEntry::Literal { path, .. } = allow else {
            continue;
        };
        if !exceptions.contains(&index)
            && mapper
                .name_spellings(path)?
                .iter()
                .any(|spelling| matcher.covers(spelling))
        {
            exceptions.push(index);
        }
    }
    exceptions.sort_unstable();
    Ok(exceptions)
}

fn read_denies(deny_read: &Region, carve_out: &Region) -> anyhow::Result<Vec<DenyRule>> {
    if deny_read.is_empty() {
        return Ok(vec![]);
    }
    let denied = Filter::RequireAny(deny_read.filters.clone());
    let not_carved = (!carve_out.is_empty()).then(|| Filter::excluding(Filter::RequireAny(carve_out.filters.clone())));
    let not_directory = Filter::excluding(Filter::VnodeType(VnodeType::Directory));
    let directory = Filter::VnodeType(VnodeType::Directory);
    let not_root = Filter::excluding(Filter::literal("/")?);
    Ok(vec![
        DenyRule::new(
            [Operation::FileReadStar],
            [Filter::RequireAll(
                [Some(denied.clone()), not_carved.clone(), Some(not_directory)]
                    .into_iter()
                    .flatten()
                    .collect(),
            )],
        ),
        // A directory stays statable by name under any deny; only listing
        // it is refused.
        DenyRule::new(
            [
                Operation::FileReadData,
                Operation::FileReadXattr,
                Operation::FileIssueExtension,
            ],
            [Filter::RequireAll(
                [Some(denied), not_carved, Some(directory), Some(not_root)]
                    .into_iter()
                    .flatten()
                    .collect(),
            )],
        ),
    ])
}

fn move_blocks(
    deny_read: &Region,
    read_groups: &[ReadGroup],
    deny_write: &Region,
    allow_write: &Region,
    mandatory_roots: &[LiteralPath],
    write_entries: &[FsPathEntry],
    mapper: &PathMapper,
) -> anyhow::Result<Vec<DenyRule>> {
    let mut rules = Vec::new();
    for group in read_groups.iter().filter(|g| !g.deny.is_empty()) {
        let mut plain: Vec<FsPathEntry> = Vec::new();
        for entry in &group.entries {
            let roots = if group.credential {
                vec![]
            } else {
                carved_write_roots(entry, write_entries)
            };
            if roots.is_empty() {
                plain.push(entry.clone());
                continue;
            }
            let own = Region::translate(std::slice::from_ref(entry), mapper)?;
            let carved = Region::translate_allows(&roots, mapper)?;
            rules.push(unlink_rule(&own, &group.carve_out, Some(&carved)));
        }
        if !plain.is_empty() {
            let own = Region::translate(&plain, mapper)?;
            rules.push(unlink_rule(&own, &group.carve_out, None));
        }
    }
    if !deny_write.is_empty() {
        rules.push(DenyRule::new([Operation::FileWriteUnlink], deny_write.filters.clone()));
    }
    for root in mandatory_roots {
        rules.push(DenyRule::new(
            [Operation::FileWriteUnlink],
            [mandatory_move_blocks(root)?],
        ));
        rules.push(DenyRule::new(
            [Operation::FileWriteCreate],
            [mandatory_create_blocks(root)?],
        ));
    }
    let protected: Vec<LiteralPath> = deny_read.anchors.iter().chain(&deny_write.anchors).cloned().collect();
    let mut pinned = pinned_ancestors(&protected, &allow_write.anchors);
    pinned.extend(pinned_bases(
        &deny_read
            .glob_bases
            .iter()
            .chain(&deny_write.glob_bases)
            .cloned()
            .collect::<Vec<_>>(),
        &allow_write.anchors,
    ));
    for group in read_groups.iter().filter(|g| g.credential) {
        pinned.extend(roots_above(
            group.deny.anchors.iter().chain(&group.deny.glob_bases),
            &allow_write.anchors,
        ));
    }
    pinned.sort();
    pinned.dedup();
    if !pinned.is_empty() {
        let filters = exact_filters(&pinned).context("pinning the ancestors of a protected path")?;
        rules.push(DenyRule::new([Operation::FileWriteUnlink], filters));
    }
    let write_protected: Vec<LiteralPath> = deny_write.anchors.clone();
    let mut write_pinned = pinned_ancestors(&write_protected, &allow_write.anchors);
    write_pinned.extend(pinned_bases(&deny_write.glob_bases, &allow_write.anchors));
    write_pinned.sort();
    write_pinned.dedup();
    if !write_pinned.is_empty() {
        let filters =
            exact_filters(&write_pinned).context("keeping links out of the ancestors of a write-protected path")?;
        rules.push(DenyRule::new([Operation::FileWriteCreate], filters));
    }
    Ok(rules)
}

fn roots_above<'a>(protected: impl Iterator<Item = &'a LiteralPath>, write_roots: &[LiteralPath]) -> Vec<LiteralPath> {
    let mut found = Vec::new();
    for path in protected {
        for ancestor in path.ancestors() {
            if ancestor.as_str() != "/" && write_roots.contains(&ancestor) {
                found.push(ancestor);
            }
        }
    }
    found
}

fn unlink_rule(deny: &Region, carve_out: &Region, write_roots: Option<&Region>) -> DenyRule {
    let mut excluded: Vec<Filter> = Vec::new();
    for filter in carve_out
        .filters
        .iter()
        .chain(write_roots.iter().flat_map(|roots| &roots.filters))
    {
        if !excluded.contains(filter) {
            excluded.push(filter.clone());
        }
    }
    let denied = Filter::RequireAny(deny.filters.clone());
    let filter = if excluded.is_empty() {
        denied
    } else {
        Filter::RequireAll(vec![denied, Filter::excluding(Filter::RequireAny(excluded))])
    };
    DenyRule::new([Operation::FileWriteUnlink], [filter])
}

/// The writable roots a read deny leaves free to change: the deny holds the
/// content back from being read, and removing a file inside a root the
/// policy made writable does not read it. A root the deny names itself
/// stays guarded when another root encloses it.
fn carved_write_roots(deny: &FsPathEntry, write_entries: &[FsPathEntry]) -> Vec<FsPathEntry> {
    let roots: Vec<&LiteralPath> = write_entries
        .iter()
        .filter_map(|entry| match entry {
            FsPathEntry::Literal { path, .. } => Some(path),
            FsPathEntry::Glob { .. } => None,
        })
        .collect();
    roots
        .iter()
        .copied()
        .filter(|root| match deny {
            FsPathEntry::Literal { path, .. } => {
                let enclosed = roots.iter().any(|other| *other != path && path.is_within(other));
                root.is_within(path) && (*root != path || !enclosed)
            }
            FsPathEntry::Glob { .. } => CompiledFsEntry::new(deny).covers(root),
        })
        .map(|root| FsPathEntry::Literal {
            path: root.clone(),
            directory: DirectoryMarker::Unmarked,
        })
        .collect()
}

#[cfg(test)]
mod tests;
