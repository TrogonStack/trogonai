use std::{io, path::Path, sync::LazyLock};

use regex::Regex;

use crate::{
    adapters::{DirectoryEntryKind, ReadDirectory, ReadFile, ReadLink, ReadSymlinkMetadata},
    constants::{
        CONNECTOR_DOWNLOAD_TEMP_PATTERN, CONNECTOR_DOWNLOADS_NAME, CONNECTOR_DOWNLOADS_ROOT,
        CONNECTOR_GITIGNORE_MARKERS, CONNECTOR_GITIGNORE_MAX_BYTES,
    },
    settings_resolve_state::ResolverState,
    settings_resolve_vetting::{Vetting, is_invisible, vet},
};

static DOWNLOAD_TEMP_NAME: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(CONNECTOR_DOWNLOAD_TEMP_PATTERN).ok());

/// Everything the connector folder check asks of its host.
pub(crate) trait ConnectorHost:
    ReadFile<Error = io::Error>
    + ReadDirectory<Error = io::Error>
    + ReadSymlinkMetadata<Error = io::Error>
    + ReadLink<Error = io::Error>
{
}

impl<T> ConnectorHost for T where
    T: ReadFile<Error = io::Error>
        + ReadDirectory<Error = io::Error>
        + ReadSymlinkMetadata<Error = io::Error>
        + ReadLink<Error = io::Error>
{
}

/// `rXe(name, true)` upstream: the spelling a folder name is compared by, lower case, without invisible characters,
/// an alternate data stream suffix, or trailing dots and spaces.
fn normalized_name(name: &str) -> String {
    let lowered = name.to_lowercase().replace('\u{131}', "i").replace('\u{17f}', "s");
    let visible: String = lowered.chars().filter(|character| !is_invisible(*character)).collect();
    let stream = visible
        .char_indices()
        .find(|(index, character)| {
            *character == ':' && !visible[index + 1..].contains(['\n', '\r', '\u{2028}', '\u{2029}'])
        })
        .map(|(index, _)| index);
    let without_stream = match stream {
        Some(index) => &visible[..index],
        None => visible.as_str(),
    };
    let trimmed = without_stream.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        lowered
    } else {
        trimmed.to_owned()
    }
}

/// `H5` upstream.
fn is_connector_name(name: &str) -> bool {
    normalized_name(name) == CONNECTOR_DOWNLOADS_NAME
}

fn components(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).collect()
}

/// `im` upstream: some component of the path names a connector download folder.
fn names_connector_folder(path: &str) -> bool {
    components(path).into_iter().any(is_connector_name)
}

/// `t7e` upstream, as a yes or no: the file is a regular file of at most three bytes that holds exactly `*`.
fn holds_ignore_marker<H: ConnectorHost>(host: &H, path: &Path) -> Result<bool, io::Error> {
    let metadata = host.symlink_metadata(path)?;
    if metadata.kind != DirectoryEntryKind::File || metadata.len > CONNECTOR_GITIGNORE_MAX_BYTES {
        return Ok(false);
    }
    let bytes = host.read_file(path)?;
    let head = &bytes[..bytes.len().min(4)];
    Ok(CONNECTOR_GITIGNORE_MARKERS.contains(&head))
}

/// `n7e` upstream: the `.gitignore` of a connector download folder ignores everything, a download left a temporary
/// file there, or anything about the folder could not be examined.
fn marks_folder_ignored<H: ConnectorHost>(host: &H, gitignore: &str) -> bool {
    let path = Path::new(gitignore);
    let Ok(vetting) = vet(host, path) else {
        return true;
    };
    let directory = match gitignore.rfind(['/', '\\']) {
        Some(index) => &gitignore[..index],
        None => gitignore,
    };
    let Ok(entries) = host.read_directory(Path::new(directory)) else {
        return true;
    };
    let download_temp = |name: &str| {
        DOWNLOAD_TEMP_NAME
            .as_ref()
            .is_some_and(|pattern| pattern.is_match(name))
    };
    if entries.iter().any(|entry| download_temp(&entry.name)) {
        return true;
    }
    if vetting == Vetting::Absent {
        return false;
    }
    holds_ignore_marker(host, path).unwrap_or(true)
}

/// `nE` upstream, remembered in `state`: some folder on the path is a connector download folder that is marked
/// ignored.
fn is_ignored_connector_path<H: ConnectorHost>(host: &H, state: &ResolverState, directory: &Path) -> bool {
    if let Some(known) = state.connector_folder(directory) {
        return known;
    }
    let text = directory.to_string_lossy().into_owned();
    let parts = components(&text);
    let mut ignored = false;
    for (index, part) in parts.iter().enumerate() {
        if ignored {
            break;
        }
        if is_connector_name(part) {
            let folder = parts[..=index].join("/");
            ignored = folder == CONNECTOR_DOWNLOADS_ROOT || marks_folder_ignored(host, &format!("{folder}/.gitignore"));
        }
    }
    state.remember_connector_folder(directory, ignored);
    ignored
}

/// Whether the project and local settings of `cwd` must not be read: the working directory sits inside a
/// connector download folder that is marked ignored (`VP` and the check in `Hu` upstream).
pub(crate) fn skips_repository_settings<H: ConnectorHost>(host: &H, state: &ResolverState, cwd: &Path) -> bool {
    names_connector_folder(&cwd.to_string_lossy()) && is_ignored_connector_path(host, state, cwd)
}
