#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WirePreflightError {
    #[error("protobuf exceeds byte limit")]
    TooLarge,
    #[error("protobuf exceeds nesting limit")]
    DepthLimit,
    #[error("protobuf exceeds field limit")]
    FieldLimit,
    #[error("protobuf varint is malformed")]
    MalformedVarint,
    #[error("protobuf value is truncated")]
    Truncated,
    #[error("protobuf tag is invalid")]
    InvalidTag,
    #[error("protobuf contains unknown field {field_number}")]
    UnknownField { field_number: u32 },
    #[error("protobuf repeats singular field {field_number}")]
    DuplicateSingular { field_number: u32 },
    #[error("protobuf selects conflicting oneof arms")]
    ConflictingOneof,
    #[error("protobuf field {field_number} has incompatible wire type")]
    WrongWireType { field_number: u32 },
    #[error("protobuf groups are unsupported")]
    UnsupportedGroup,
    #[error("protobuf maps require a separately reviewed duplicate-key profile")]
    UnsupportedMap,
    #[error("map profile belongs to another schema")]
    MapProfileSchema,
    #[error("protobuf map field {field_number} repeats a logical key")]
    DuplicateMapKey { field_number: u32 },
    #[error("protobuf field {field_number} has invalid UTF-8")]
    InvalidUtf8 { field_number: u32 },
    #[error("protobuf field {field_number} has invalid boolean")]
    InvalidBoolean { field_number: u32 },
    #[error("protobuf field {field_number} has an unknown enum value")]
    UnknownEnum { field_number: u32 },
    #[error("protobuf field {field_number} has an out-of-range integer")]
    InvalidInteger { field_number: u32 },
    #[error("protobuf is missing required field {field_number}")]
    MissingRequired { field_number: u32 },
}
