use async_nats::jetstream::{
    ErrorCode,
    context::{CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind},
    kv::{CreateError, CreateErrorKind},
};

pub fn is_create_stream_already_exists(error: &CreateStreamError) -> bool {
    matches!(
        error.kind(),
        CreateStreamErrorKind::JetStream(ref source)
            if source.error_code() == ErrorCode::STREAM_NAME_EXIST
    )
}

pub fn is_create_key_value_already_exists(error: &CreateKeyValueError) -> bool {
    if error.kind() != CreateKeyValueErrorKind::BucketCreate {
        return false;
    }

    std::error::Error::source(error)
        .and_then(|source| source.downcast_ref::<CreateStreamError>())
        .is_some_and(is_create_stream_already_exists)
}

/// A `create` that lost the key to somebody else, as opposed to one that never
/// reached the bucket. The only conflict a caller reserving a key can act on:
/// the key is taken, so there is a winner to read, whereas any other failure
/// says nothing about what is stored.
pub fn is_create_key_already_exists(error: &CreateError) -> bool {
    error.kind() == CreateErrorKind::AlreadyExists
}

#[cfg(test)]
mod tests;
