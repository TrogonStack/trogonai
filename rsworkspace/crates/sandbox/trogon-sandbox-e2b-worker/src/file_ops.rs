//! Structured file operations served through envd's filesystem service and a few guest shell helpers.

mod api_failure;
mod archive_get;
mod archive_put;
mod archive_upload;
mod checksum;
mod collection_source;
mod constants;
mod edit;
mod file_guest;
mod guest_bodies;
mod guest_entry;
mod guest_entry_error;
mod list;
mod path;
mod payload;
mod read;
mod script_failure;
mod search;
mod search_spec;
mod staging;
mod stderr_kind;
mod tar_size;
#[cfg(test)]
mod tests;
mod write;
mod write_spec;

use trogon_sandbox_contract::{FileOp, FileOpResponse};
use trogon_sandbox_file_ops::FileOpError;

use self::archive_get::archive_get;
use self::archive_put::archive_put;
use self::checksum::checksum;
use self::edit::edit;
use self::file_guest::FileGuest;
use self::list::list;
use self::path::{mkdir, remove, rename};
use self::payload::decode;
use self::read::read;
use self::search::search;
use self::search_spec::SearchSpec;
use self::write::write;
use self::write_spec::WriteSpec;
use crate::envd::{Envd, EnvdTarget};

/// Serves one file operation against the sandbox behind `target`, acting as `user`.
pub(crate) async fn execute(envd: &Envd, target: &EnvdTarget, user: &str, op: &FileOp) -> FileOpResponse {
    let guest = FileGuest::new(envd, target, user);
    serve(&guest, op).await.unwrap_or_else(FileOpResponse::from)
}

async fn serve(guest: &FileGuest<'_>, op: &FileOp) -> Result<FileOpResponse, FileOpError> {
    match op {
        FileOp::Stat { path } => Ok(FileOpResponse::Stat {
            stat: guest.stat(path).await?,
        }),
        FileOp::List {
            path,
            recursive,
            max_entries,
        } => list(guest, path, *recursive, *max_entries).await,
        FileOp::Read {
            path,
            offset,
            length,
            max_bytes,
        } => read(guest, path, *offset, *length, *max_bytes).await,
        FileOp::Write {
            path,
            data,
            offset,
            truncate,
            mode,
            create_parents,
        } => {
            let spec = WriteSpec {
                path,
                data,
                offset: *offset,
                truncate: *truncate,
                mode: *mode,
                create_parents: *create_parents,
            };
            write(guest, &spec).await
        }
        FileOp::Edit {
            path,
            find,
            replace,
            expected_occurrences,
        } => edit(guest, path, find, replace, *expected_occurrences).await,
        FileOp::Search {
            path,
            glob,
            pattern,
            regex,
            case_insensitive,
            max_results,
            max_file_bytes,
        } => {
            let spec = SearchSpec {
                path,
                glob: glob.as_deref(),
                searches_content: pattern.is_some(),
                max_file_bytes: *max_file_bytes,
                staged: FileOp::Search {
                    path: "/".into(),
                    glob: glob.clone(),
                    pattern: pattern.clone(),
                    regex: *regex,
                    case_insensitive: *case_insensitive,
                    max_results: *max_results,
                    max_file_bytes: *max_file_bytes,
                },
            };
            search(guest, &spec).await
        }
        FileOp::ArchiveGet { path, max_bytes } => archive_get(guest, path, *max_bytes).await,
        FileOp::ArchivePut { path, data } => archive_put(guest, path, &decode(data)).await,
        FileOp::Checksum { path, offset, length } => checksum(guest, path, *offset, *length).await,
        FileOp::Mkdir { path, parents } => mkdir(guest, path, *parents).await,
        FileOp::Remove { path, recursive } => remove(guest, path, *recursive).await,
        FileOp::Rename { from, to, overwrite } => rename(guest, from, to, *overwrite).await,
        FileOp::Checkpoint { .. }
        | FileOp::Changes { .. }
        | FileOp::WatchStart { .. }
        | FileOp::WatchPoll { .. }
        | FileOp::WatchStop { .. } => Err(FileOpError::Unsupported {
            message: "e2b serves this file operation through its tracking and watch state".into(),
        }),
    }
}
