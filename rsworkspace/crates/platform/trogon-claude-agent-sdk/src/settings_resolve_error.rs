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
    /// The `RangeError` `resolveSettings` rejects with in the TypeScript dialect when copying a deeply nested
    /// settings file into its file cache, or merging it, runs out of the stack [`ResolverState::with_node_stack`](crate::ResolverState::with_node_stack)
    /// describes.
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
    /// The `SyntaxError` `resolveSettings` rejects with in the TypeScript dialect when V8 runs out of the stack
    /// [`ResolverState::with_node_stack`](crate::ResolverState::with_node_stack) describes while it compiles the regular expression lodash `isIndex` tests
    /// the indexes of a merged array with, which it does on its first two tests in the process.
    #[error("Invalid regular expression: /^(?:0|[1-9]\\d*)$/: Stack overflow")]
    InvalidRegularExpression,
}
