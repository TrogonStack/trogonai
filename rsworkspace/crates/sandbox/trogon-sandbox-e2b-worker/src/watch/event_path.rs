/// The absolute path of an event: envd names it relative to the watched directory.
pub(super) fn event_path(root: &str, name: &str) -> String {
    if name.starts_with('/') {
        return name.to_owned();
    }
    let name = name.trim_start_matches("./");
    let root = root.trim_end_matches('/');
    if name.is_empty() {
        return if root.is_empty() {
            "/".to_owned()
        } else {
            root.to_owned()
        };
    }
    format!("{root}/{name}")
}
