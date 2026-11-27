//! Windows paths in the one spelling a filesystem policy has: absolute and
//! `/`-separated. A drive path `C:\a\b` is `/C:/a/b`, and a network path
//! `\\server\share\a` is `/UNC/server/share/a`.

use crate::constants::UNC_ROOT;

fn is_separator(ch: char) -> bool {
    ch == '/' || ch == '\\'
}

/// The policy spelling of a Windows path, or `None` when `text` is not an
/// absolute Windows path. The `\\?\` and `\\.\` prefixes of the extended form
/// are dropped.
pub fn internal(text: &str) -> Option<String> {
    let extended = text.strip_prefix(r"\\?\").or_else(|| text.strip_prefix(r"\\.\"));
    let unc = match extended {
        Some(rest) => match rest.strip_prefix(r"UNC\") {
            Some(share) => format!(r"\\{share}"),
            None => rest.to_string(),
        },
        None => text.to_string(),
    };
    let text = unc.as_str();
    let bytes = text.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        if !is_separator(char::from(bytes[2])) {
            return None;
        }
        let drive = char::from(bytes[0]).to_ascii_uppercase();
        let rest = text[3..].replace('\\', "/");
        let rest = rest.trim_end_matches('/');
        return Some(if rest.is_empty() {
            format!("/{drive}:/")
        } else {
            format!("/{drive}:/{rest}")
        });
    }
    if bytes.len() >= 2 && is_separator(char::from(bytes[0])) && is_separator(char::from(bytes[1])) {
        let rest = text[2..].replace('\\', "/");
        let rest = rest.trim_end_matches('/');
        return Some(format!("/{UNC_ROOT}/{rest}"));
    }
    None
}

/// The Windows spelling of a policy path made by [`internal`], or `None` when
/// it is not one.
pub fn native(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix(&format!("/{UNC_ROOT}/")) {
        return Some(format!(r"\\{}", rest.replace('/', r"\")));
    }
    let rest = path.strip_prefix('/')?;
    let bytes = rest.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    if bytes.len() > 2 && bytes[2] != b'/' {
        return None;
    }
    let tail = rest[2..].replace('/', r"\");
    Some(if tail.is_empty() {
        format!("{}:{}", &rest[..1], r"\")
    } else {
        format!("{}:{tail}", &rest[..1])
    })
}

pub(crate) fn key(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

pub(crate) fn is_beneath(path: &str, dir: &str) -> bool {
    path != dir && path.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))
}

/// The topmost grants strictly beneath each deny, which rank above those denies.
/// Paths are compared without regard to case or separator.
pub fn grants_beneath_denies(grants: &[String], denies: &[String]) -> Vec<String> {
    grants_beneath_denies_by(grants, denies, |path| path.to_string())
}

/// [`grants_beneath_denies`] comparing the paths `resolve` returns, so that a
/// link, a junction or an 8.3 name is compared as the object it leads to
/// (the comparison key of a path).
pub fn grants_beneath_denies_by(grants: &[String], denies: &[String], resolve: impl Fn(&str) -> String) -> Vec<String> {
    let mut out: Vec<(String, String)> = Vec::new();
    for deny in denies.iter().map(|deny| key(&resolve(deny))) {
        let beneath: Vec<(String, &String)> = grants
            .iter()
            .map(|grant| (key(&resolve(grant)), grant))
            .filter(|(grant, _)| is_beneath(grant, &deny))
            .collect();
        for (grant, original) in &beneath {
            let covered = beneath.iter().any(|(other, _)| is_beneath(grant, other));
            if !covered && !out.iter().any(|(seen, _)| seen == grant) {
                out.push((grant.clone(), (*original).clone()));
            }
        }
    }
    out.into_iter().map(|(_, original)| original).collect()
}

#[cfg(test)]
mod tests;
