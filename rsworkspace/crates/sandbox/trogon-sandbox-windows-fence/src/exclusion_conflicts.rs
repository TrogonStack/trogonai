use crate::access::Access;
use crate::dir_look::DirLook;
use crate::exclusion_conflict::ExclusionConflict;
use crate::exclusion_kind::ExclusionKind;
use crate::path_on_disk::PathOnDisk;
use crate::profile_disk::ProfileDisk;
use crate::profile_exclusions::ProfileExclusions;
use crate::relation::Relation;

fn strip_extended(text: &str) -> String {
    if let Some(rest) = text
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| text.strip_prefix(r"\\.\UNC\"))
    {
        return format!(r"\\{rest}");
    }
    text.strip_prefix(r"\\?\")
        .or_else(|| text.strip_prefix(r"\\.\"))
        .unwrap_or(text)
        .to_string()
}

/// A path as lower case components: `/` separators, no `.`, `..` or empty
/// component, no separator at the end.
fn parts_of(text: &str) -> Vec<String> {
    let text = strip_extended(text).replace('\\', "/");
    let network = text.starts_with("//");
    let mut out: Vec<String> = Vec::new();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other.to_lowercase()),
        }
    }
    if network {
        out.splice(0..0, [String::new(), String::new()]);
    }
    out
}

fn is_unprobed(text: &str) -> bool {
    let text = text.replace('/', r"\");
    let bytes = text.as_bytes();
    if !text.starts_with(r"\\") {
        return false;
    }
    if matches!(bytes.get(2), Some(b'?' | b'.')) && bytes.get(3) == Some(&b'\\') {
        return !matches!(bytes.get(4..6), Some([drive, b':']) if drive.is_ascii_alphabetic());
    }
    true
}

pub(crate) fn resolve(path: &str, disk: &dyn ProfileDisk) -> PathOnDisk {
    if is_unprobed(path) {
        PathOnDisk::Unprobed
    } else {
        disk.resolve(path)
    }
}

fn key_parts(path: &str, on_disk: &PathOnDisk) -> Vec<String> {
    match on_disk {
        PathOnDisk::Resolved(real) => parts_of(real),
        _ => parts_of(path),
    }
}

fn component_matches(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

enum Beneath {
    None,
    Match,
    Unexamined(String),
}

fn matched_beneath(dir: &str, rest: &[String], top: bool, disk: &dyn ProfileDisk) -> Beneath {
    if rest.is_empty() {
        return Beneath::Match;
    }
    let names = match disk.look(dir) {
        DirLook::Names(names) => names,
        DirLook::Link if top => return Beneath::Unexamined(format!("'{dir}' is a link")),
        DirLook::Link | DirLook::Absent | DirLook::NotDirectory => return Beneath::None,
        DirLook::Failed(code) => {
            return Beneath::Unexamined(format!("'{dir}' could not be listed ({code})"));
        }
    };
    let mut unexamined = None;
    for name in names {
        if !component_matches(&rest[0], &name.to_lowercase()) {
            continue;
        }
        let child = format!(r"{}\{name}", dir.trim_end_matches('\\'));
        match matched_beneath(&child, &rest[1..], false, disk) {
            Beneath::Match => return Beneath::Match,
            Beneath::Unexamined(reason) => unexamined = unexamined.or(Some(reason)),
            Beneath::None => {}
        }
    }
    unexamined.map_or(Beneath::None, Beneath::Unexamined)
}

struct Rule<'a> {
    exclusion: &'a str,
    kind: ExclusionKind,
    parts: Vec<String>,
    only_equal: bool,
}

fn rules_of<'a>(list: &'a [String], kind: ExclusionKind, only_equal: bool) -> Vec<Rule<'a>> {
    list.iter()
        .map(|exclusion| Rule {
            exclusion,
            kind,
            parts: parts_of(exclusion),
            only_equal,
        })
        .collect()
}

/// The paths among `paths` a session
/// must not grant while the profile read grant exists, with the exclusion each
/// collides with. Paths are resolved first; a path that exists and cannot be
/// listed fails closed. `admitted` names read-granted paths the embedder lets
/// reach a protected exclusion; it does not apply to a write.
pub fn profile_exclusion_conflicts(
    paths: &[String],
    grant: &ProfileExclusions,
    access: Access,
    admitted: &[String],
    disk: &dyn ProfileDisk,
) -> Vec<ExclusionConflict> {
    if paths.is_empty() {
        return Vec::new();
    }
    let mut rules = rules_of(&grant.ungranted_only, ExclusionKind::UngrantedOnly, true);
    rules.extend(rules_of(&grant.exclusions, ExclusionKind::Protected, false));
    rules.extend(rules_of(&grant.ungranted_only, ExclusionKind::UngrantedOnly, false));
    if rules.is_empty() {
        return Vec::new();
    }
    let admitted: Vec<Vec<String>> = if access == Access::Write {
        Vec::new()
    } else {
        admitted
            .iter()
            .map(|path| key_parts(path, &resolve(path, disk)))
            .collect()
    };
    let mut out = Vec::new();
    for path in paths {
        let on_disk = resolve(path, disk);
        let parts = key_parts(path, &on_disk);
        for rule in &rules {
            let shared = parts.len().min(rule.parts.len());
            let related = (0..shared).all(|i| component_matches(&rule.parts[i], &parts[i]));
            if !related {
                continue;
            }
            let relation = match parts.len().cmp(&rule.parts.len()) {
                std::cmp::Ordering::Equal => Relation::Equal,
                std::cmp::Ordering::Greater => Relation::Inside,
                std::cmp::Ordering::Less => Relation::Above,
            };
            if rule.only_equal && relation != Relation::Equal {
                continue;
            }
            let mut unexamined = None;
            if relation == Relation::Above && rule.exclusion.contains('*') {
                match &on_disk {
                    PathOnDisk::Absent => continue,
                    PathOnDisk::Resolved(real) => match matched_beneath(real, &rule.parts[parts.len()..], true, disk) {
                        Beneath::None => continue,
                        Beneath::Match => {}
                        Beneath::Unexamined(reason) => unexamined = Some(reason),
                    },
                    PathOnDisk::Unprobed => {
                        unexamined = Some(format!("'{path}' is not on a local drive and is not examined"));
                    }
                    PathOnDisk::Failed(code) => {
                        unexamined = Some(format!("'{path}' could not be resolved ({code})"));
                    }
                }
            }
            if rule.kind == ExclusionKind::UngrantedOnly {
                if relation == Relation::Inside {
                    continue;
                }
            } else if relation != Relation::Above && admitted.contains(&parts) {
                continue;
            }
            out.push(ExclusionConflict {
                path: path.clone(),
                exclusion: rule.exclusion.to_string(),
                kind: rule.kind,
                access,
                relation,
                unexamined,
            });
            break;
        }
    }
    out
}
