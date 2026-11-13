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
