use trogon_sandbox_contract::FileOp;

/// Every sandbox path an operation names, so the root policy can rewrite each one.
pub(super) fn file_op_paths_mut(op: &mut FileOp) -> Vec<&mut String> {
    match op {
        FileOp::List { path, .. }
        | FileOp::Stat { path, .. }
        | FileOp::Read { path, .. }
        | FileOp::Write { path, .. }
        | FileOp::Edit { path, .. }
        | FileOp::Search { path, .. }
        | FileOp::Remove { path, .. }
        | FileOp::Mkdir { path, .. }
        | FileOp::ArchiveGet { path, .. }
        | FileOp::ArchivePut { path, .. }
        | FileOp::Checkpoint { path, .. }
        | FileOp::Checksum { path, .. }
        | FileOp::WatchStart { path, .. } => vec![path],
        FileOp::Changes { path, .. } => path.iter_mut().collect(),
        FileOp::Rename { from, to, .. } => vec![from, to],
        FileOp::WatchPoll { .. } | FileOp::WatchStop { .. } => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
