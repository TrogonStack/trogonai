use serde::Serialize;

/// Body of the filesystem calls that name one path.
#[derive(Serialize)]
pub(super) struct PathBody<'a> {
    pub(super) path: &'a str,
}

/// Body of `ListDir`.
#[derive(Serialize)]
pub(super) struct ListDirBody<'a> {
    pub(super) path: &'a str,
    pub(super) depth: u32,
}

/// Body of `Move`.
#[derive(Serialize)]
pub(super) struct MoveBody<'a> {
    pub(super) source: &'a str,
    pub(super) destination: &'a str,
}
