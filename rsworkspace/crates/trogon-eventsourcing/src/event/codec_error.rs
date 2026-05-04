#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError<DataError, MetadataError> {
    Data(DataError),
    Metadata(MetadataError),
}
