pub(super) const LINK_DEPTH: u32 = 40;

pub(super) const PARTIAL_DEPTH: usize = 6;

pub(super) const AUTOFS_TYPE: i64 = 391;

pub(super) const ASKED_TWICE: u32 = 2;

pub(super) const EXPECTED_ERRORS: [&str; 7] = [
    "ENOENT",
    "ENOTDIR",
    "EACCES",
    "EPERM",
    "ENOSYS",
    "ENOTSUP",
    "EOPNOTSUPP",
];

pub(super) const DATA_VOLUME_ROOTS: [&str; 16] = [
    "usr/local",
    "usr/libexec/cups",
    "usr/share/snmp",
    "AppleInternal",
    "Applications",
    "Library",
    "Users",
    "Volumes",
    "cores",
    "home",
    "media",
    "mnt",
    "opt",
    "pkg",
    "private",
    "sw",
];
