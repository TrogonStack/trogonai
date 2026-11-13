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
mod tests {
    use super::uid_for_name;

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/sh\ndaemon:*:1:1::/:/usr/sbin/nologin\nbad:x:notanumber:0\n";

    #[test]
    fn finds_the_uid_of_an_exact_name() {
        assert_eq!(uid_for_name(PASSWD, "root"), Some(0));
        assert_eq!(uid_for_name(PASSWD, "daemon"), Some(1));
    }

    #[test]
    fn rejects_unknown_prefix_and_malformed_entries() {
        assert_eq!(uid_for_name(PASSWD, "roo"), None);
        assert_eq!(uid_for_name(PASSWD, "bad"), None);
        assert_eq!(uid_for_name(PASSWD, "nobody"), None);
    }
}
