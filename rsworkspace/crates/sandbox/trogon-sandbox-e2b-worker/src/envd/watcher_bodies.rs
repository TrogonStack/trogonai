use serde::Serialize;

/// Body of `filesystem.Filesystem/CreateWatcher`. Entries are requested so a create or write
/// event says whether the path is a directory.
#[derive(Serialize)]
pub(super) struct CreateWatcherBody<'a> {
    pub(super) path: &'a str,
    pub(super) recursive: bool,
    #[serde(rename = "includeEntry")]
    pub(super) include_entry: bool,
}

/// Body of `GetWatcherEvents` and `RemoveWatcher`.
#[derive(Serialize)]
pub(super) struct WatcherIdBody<'a> {
    #[serde(rename = "watcherId")]
    pub(super) watcher_id: &'a str,
}
