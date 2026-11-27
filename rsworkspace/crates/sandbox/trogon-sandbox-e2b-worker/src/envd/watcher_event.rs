use trogon_sandbox_contract::FileWatchEventKind;

/// One change envd reported, already in the platform's event vocabulary.
///
/// Mapping from `filesystem.EventType`: CREATE is created; WRITE and CHMOD are modified;
/// REMOVE and RENAME are deleted (a rename reports the old name as gone and the new one as
/// a CREATE). `EVENT_TYPE_UNSPECIFIED` and values this worker does not know are skipped.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WatcherEvent {
    /// `FilesystemEvent.name`: the changed path relative to the watched directory.
    pub name: String,
    pub kind: FileWatchEventKind,
    /// From the event's entry, which envd omits for a removed path (then false).
    pub is_dir: bool,
}

impl WatcherEvent {
    /// Reads one envd event; `None` when its type is not one this worker reports.
    pub(super) fn from_value(event: &serde_json::Value) -> Option<Self> {
        let kind = match event.get("type")? {
            serde_json::Value::String(name) => match name.as_str() {
                "EVENT_TYPE_CREATE" => FileWatchEventKind::Created,
                "EVENT_TYPE_WRITE" | "EVENT_TYPE_CHMOD" => FileWatchEventKind::Modified,
                "EVENT_TYPE_REMOVE" | "EVENT_TYPE_RENAME" => FileWatchEventKind::Deleted,
                _ => return None,
            },
            serde_json::Value::Number(number) => match number.as_u64()? {
                1 => FileWatchEventKind::Created,
                2 | 5 => FileWatchEventKind::Modified,
                3 | 4 => FileWatchEventKind::Deleted,
                _ => return None,
            },
            _ => return None,
        };
        let is_dir = event
            .get("entry")
            .and_then(|entry| entry.get("type"))
            .is_some_and(|kind| kind == "FILE_TYPE_DIRECTORY" || kind == 2);
        let name = event
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Some(Self { name, kind, is_dir })
    }
}
