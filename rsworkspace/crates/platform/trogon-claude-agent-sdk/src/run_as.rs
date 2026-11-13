//! Resolution of Python's `ClaudeAgentOptions.user` (passed to `open_process(user=...)`, which is
//! `Popen(user=str)`: the name is looked up with `getpwnam` and only the real and effective uid change).

/// Finds the uid of `name` in the text of a passwd database (`name:password:uid:...`).
pub(crate) fn uid_for_name(passwd: &str, name: &str) -> Option<u32> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next() == Some(name))
            .then(|| fields.nth(1).and_then(|uid| uid.parse().ok()))
            .flatten()
    })
}

#[cfg(unix)]
pub(crate) fn lookup(name: &str) -> std::io::Result<u32> {
    // Name service switch modules are not consulted; `/etc/passwd` is the portable local source.
    let passwd = std::fs::read_to_string("/etc/passwd")?;
    uid_for_name(&passwd, name)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("unknown user: {name}")))
}

#[cfg(test)]
mod tests;
