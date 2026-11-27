use trogon_sandbox_contract::FileOpResponse;
use trogon_sandbox_file_ops::FileOpError;

/// A poll, start or stop outcome: the reply, or the failure that stands for one.
pub(super) type WatchOutcome = Result<FileOpResponse, FileOpError>;
