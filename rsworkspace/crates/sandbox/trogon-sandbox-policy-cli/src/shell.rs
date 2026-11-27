#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::PathBuf;

/// The first executable called `name` in the directories of `path`.
#[cfg(unix)]
pub fn find_in_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// Single quotes everywhere except for a word made only of characters no
/// POSIX shell treats specially. A leading `=` is quoted because zsh expands
/// it, and `%` is quoted because zsh reads it in prompts and job specs.
pub fn quote(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    let mut chars = arg.chars();
    let first_is_bare = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || "_./:@+,-".contains(c));
    if first_is_bare && chars.all(|c| c.is_ascii_alphanumeric() || "_./:=@+,-".contains(c)) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\"'\"'"))
    }
}

pub fn join(args: &[String]) -> String {
    args.iter().map(|arg| quote(arg)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
