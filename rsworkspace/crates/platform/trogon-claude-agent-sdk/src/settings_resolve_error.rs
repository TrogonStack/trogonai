/// Why [`resolve_settings`](crate::resolve_settings) could not produce a result.
#[derive(Debug, thiserror::Error)]
pub enum ResolveSettingsError {
    #[error("the working directory could not be read")]
    CurrentDirectory(#[source] std::io::Error),
    #[error("the resolved settings do not fit the declared types")]
    Decode(#[source] serde_json::Error),
    #[error("the settings reader task did not finish")]
    Join(#[source] tokio::task::JoinError),
    #[error("the user settings directory needs a home directory")]
    HomeDirectory(#[source] crate::HomeDirectoryError),
    #[error("the settings reader thread could not start")]
    Thread(#[source] std::io::Error),
    /// The `RangeError` `resolveSettings` rejects with in the TypeScript dialect when a settings document is nested
    /// [`SETTINGS_DEPTH_BOUND`](crate::SETTINGS_DEPTH_BOUND) levels (500) or deeper, as Node runs out of stack
    /// reading, copying and merging it.
    ///
    /// Node fails at a depth that depends on the tier V8 runs each function in, which functions it has compiled,
    /// the stack of the caller and what the process ran before, so no depth is the same in two runs. The SDK
    /// fails every document at the bound instead, on every host and for every state: the largest multiple of 100
    /// within half the smallest margin any recorded Node run leaves. The bound holds for every settings document
    /// the TypeScript dialect reads: the user, project and local settings, the managed settings file and its
    /// drop-ins, the machine policy read from the profile, the property list or the registry, and the managed and
    /// server-managed settings passed in the options. The Python dialect has no bound of this kind.
    #[error("Maximum call stack size exceeded")]
    MaximumCallStack,
    /// The `RangeError` `resolveSettings` rejects with in the TypeScript dialect when a merge sets the `length` of
    /// an array to a value that is not a whole number from 0 to 2^32 - 1, or merges an array onto an object whose
    /// `length` passes 2^32 - 1.
    #[error("Invalid array length")]
    InvalidArrayLength,
    /// The `RangeError` the TypeScript dialect fails with where building a string passes what a V8 string holds:
    /// where a merge sets the `length` of an array to an array that joins that long, where
    /// [`resolve_settings_json_with_state`](crate::resolve_settings_json_with_state) prints a text that long as `JSON.stringify` throws it, and in the typed
    /// resolves where the holes merges leave in arrays alone would print that long, one `null` each.
    #[error("Invalid string length")]
    InvalidStringLength,
}
