/// A string is not an S3 bucket name [`S3BucketName`](crate::S3BucketName) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("'{0}' is not a valid S3 bucket name")]
pub struct S3BucketNameError(pub String);
