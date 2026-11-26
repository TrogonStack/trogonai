/// The one spelling each proxy variable has in a sandboxed command's
/// environment.
const PROXY_NAME_SPELLING: [&str; 7] = [
    "http_proxy",
    "https_proxy",
    "ftp_proxy",
    "all_proxy",
    "no_proxy",
    "grpc_proxy",
    "RSYNC_PROXY",
];

/// Merge environment layers ignoring case, as Windows reads names: a later
/// layer's entry replaces an earlier one, name and value, keeping the place of
/// the first. Two names that differ only by case in one block make .NET's
/// `Process.Start` throw.
pub fn merge_windows_env<'a>(layers: impl IntoIterator<Item = &'a [(String, String)]>) -> Vec<(String, String)> {
    let mut merged: Vec<(String, (String, String))> = Vec::new();
    for layer in layers {
        for (name, value) in layer {
            let folded = name.to_uppercase();
            let spelling = PROXY_NAME_SPELLING
                .iter()
                .find(|n| n.to_uppercase() == folded)
                .map_or(name.as_str(), |n| *n)
                .to_string();
            match merged.iter_mut().find(|(f, _)| *f == folded) {
                Some((_, slot)) => *slot = (spelling, value.clone()),
                None => merged.push((folded, (spelling, value.clone()))),
            }
        }
    }
    merged.into_iter().map(|(_, kv)| kv).collect()
}
