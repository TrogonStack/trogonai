use trogon_sandbox_contract::{
    FileCaseSensitivity, FileKind, FileOp, FileOpResponse, FileOverwrite, FileParentCreation, FilePresence,
    FileRecursion, FileSearchSyntax, SearchMatch,
};

use crate::check_failure::ensure;
use crate::held::Held;
use crate::{CheckFailureError, CheckFuture, outcomes};

fn is_missing(reply: &FileOpResponse) -> bool {
    matches!(reply, FileOpResponse::NotFound { .. } | FileOpResponse::Failed { .. })
}

async fn mkdir(held: &Held<'_>, path: &str) -> Result<FileOpResponse, CheckFailureError> {
    held.file_op(FileOp::Mkdir {
        path: path.to_string(),
        parents: FileParentCreation::Create,
    })
    .await
}

async fn remove_tree(held: &Held<'_>, path: String) -> Result<FileOpResponse, CheckFailureError> {
    held.file_op(FileOp::Remove {
        path,
        recursive: FileRecursion::Recursive,
    })
    .await
}

async fn stat(held: &Held<'_>, path: &str) -> Result<FileOpResponse, CheckFailureError> {
    held.file_op(FileOp::Stat { path: path.to_string() }).await
}

async fn search(
    held: &Held<'_>,
    path: &str,
    glob: Option<&str>,
    pattern: Option<&str>,
) -> Result<FileOpResponse, CheckFailureError> {
    held.file_op(FileOp::Search {
        path: path.to_string(),
        glob: glob.map(str::to_string),
        pattern: pattern.map(str::to_string),
        regex: FileSearchSyntax::Literal,
        case_insensitive: FileCaseSensitivity::Sensitive,
        max_results: None,
        max_file_bytes: None,
    })
    .await
}

fn created(reply: &FileOpResponse, existed: FilePresence) -> bool {
    matches!(reply, FileOpResponse::Created { already_existed } if *already_existed == existed)
}

fn removed(reply: &FileOpResponse, existed: FilePresence) -> bool {
    matches!(reply, FileOpResponse::Removed { existed: found } if *found == existed)
}

fn describes(reply: &FileOpResponse, kind: FileKind, size: Option<u64>) -> bool {
    matches!(reply, FileOpResponse::Stat { stat } if stat.kind == kind && size.is_none_or(|size| stat.size == size))
}

fn lists(reply: &FileOpResponse, names: &[&str]) -> bool {
    matches!(reply, FileOpResponse::Listing { entries, .. } if names.iter().all(|name| entries.iter().any(|e| e.name == *name)))
}

fn found_only(reply: &FileOpResponse, name: &str) -> bool {
    matches!(reply, FileOpResponse::SearchResults { matches, .. } if matches.len() == 1 && matches[0].path.ends_with(name))
}

fn found_needle(reply: &FileOpResponse, name: &str) -> bool {
    matches!(reply, FileOpResponse::SearchResults { matches, .. } if matches.len() == 2 && on_line_two(matches, name))
}

fn on_line_two(matches: &[SearchMatch], name: &str) -> bool {
    matches.iter().any(|m| m.path.ends_with(name) && m.line == Some(2))
}

pub(crate) fn file_verbs_round_trip<'a>(held: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let dir = held.scratch();
        let file = format!("{dir}/a.txt");
        let moved = format!("{dir}/b.txt");
        let nested = format!("{dir}/sub/deep");
        let absent = FilePresence::Absent;
        let present = FilePresence::Present;

        let made = mkdir(held, &dir).await?;
        ensure(created(&made, absent), "mkdir to create the directory", &made)?;
        let made = mkdir(held, &nested).await?;
        ensure(created(&made, absent), "a nested mkdir to create", &made)?;
        let made = mkdir(held, &nested).await?;
        ensure(created(&made, present), "a repeated mkdir to find it", &made)?;

        held.write_text(&file, "hello files\n").await?;
        let read = held.read_text(&file).await?;
        ensure(read == "hello files\n", "the written text back", &read)?;

        let statted = stat(held, &file).await?;
        let file_ok = describes(&statted, FileKind::File, Some(12));
        ensure(file_ok, "stat to describe the file", &statted)?;
        let statted = stat(held, &dir).await?;
        let dir_ok = describes(&statted, FileKind::Directory, None);
        ensure(dir_ok, "stat to describe the directory", &statted)?;

        let listing = FileOp::List {
            path: dir.clone(),
            recursive: FileRecursion::Flat,
            max_entries: None,
        };
        let listed = held.file_op(listing).await?;
        let listing_ok = lists(&listed, &["a.txt", "sub"]);
        ensure(listing_ok, "a listing with a.txt and sub", &listed)?;

        let rename = FileOp::Rename {
            from: file.clone(),
            to: moved.clone(),
            overwrite: FileOverwrite::Refuse,
        };
        let renamed = held.file_op(rename).await?;
        let moved_ok = matches!(renamed, FileOpResponse::Renamed);
        ensure(moved_ok, "rename to succeed", &renamed)?;
        let after = stat(held, &file).await?;
        ensure(is_missing(&after), "the renamed source to be gone", &after)?;
        let read = held.read_text(&moved).await?;
        ensure(read == "hello files\n", "the renamed file to keep its text", &read)?;

        let removal = FileOp::Remove {
            path: moved.clone(),
            recursive: FileRecursion::Flat,
        };
        let gone = held.file_op(removal).await?;
        ensure(removed(&gone, present), "remove to find the file", &gone)?;
        let gone = stat(held, &moved).await?;
        ensure(is_missing(&gone), "the removed file to be gone", &gone)?;
        let gone = remove_tree(held, dir).await?;
        ensure(removed(&gone, present), "a recursive remove to find it", &gone)
    })
}

pub(crate) fn file_verbs_report_missing_paths<'a>(held: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let path = format!("{}/absent.txt", held.scratch());
        let reading = FileOp::Read {
            path: path.clone(),
            offset: 0,
            length: None,
            max_bytes: None,
        };
        let read = held.file_op(reading).await?;
        ensure(is_missing(&read), "a read of a missing file to find nothing", &read)?;
        let statted = stat(held, &path).await?;
        ensure(
            is_missing(&statted),
            "a stat of a missing file to find nothing",
            &statted,
        )
    })
}

pub(crate) fn file_search_finds_names_and_content<'a>(held: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let dir = held.scratch();
        held.write_text(&format!("{dir}/one.txt"), "alpha\nneedle here\n")
            .await?;
        held.write_text(&format!("{dir}/two.log"), "needle in a log\n").await?;

        let by_name = search(held, &dir, Some("*.txt"), None).await?;
        let glob_ok = found_only(&by_name, "one.txt");
        ensure(glob_ok, "a glob search to find one.txt only", &by_name)?;

        let by_content = search(held, &dir, None, Some("needle")).await?;
        let content_ok = found_needle(&by_content, "one.txt");
        ensure(content_ok, "needle in both files", &by_content)?;
        let _ = remove_tree(held, dir).await;
        Ok(())
    })
}

pub(crate) fn file_archive_round_trips_a_tree<'a>(held: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let src = held.scratch();
        let dst = held.scratch();
        held.write_text(&format!("{src}/top.txt"), "top").await?;
        held.write_text(&format!("{src}/inner/leaf.txt"), "leaf").await?;

        let archive = held
            .file_op(FileOp::ArchiveGet {
                path: src.clone(),
                max_bytes: None,
            })
            .await?;
        let data = outcomes::archive(archive)?;
        let extracted = held
            .file_op(FileOp::ArchivePut {
                path: dst.clone(),
                data,
            })
            .await?;
        let unpacked = outcomes::extracted(&extracted);
        ensure(unpacked, "an extraction with entries", &extracted)?;
        let leaf = held.read_text(&format!("{dst}/inner/leaf.txt")).await?;
        ensure(leaf == "leaf", "the extracted leaf to read leaf", &leaf)?;
        let _ = remove_tree(held, src).await;
        let _ = remove_tree(held, dst).await;
        Ok(())
    })
}
