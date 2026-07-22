use async_nats::jetstream::{
    ErrorCode,
    context::{CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind},
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

#[cfg(test)]
mod tests;
