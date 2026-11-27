/// Whether users other than the mounting user may access a FUSE mount.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum FuseOtherUsers {
    #[default]
    Denied,
    Allowed,
}
