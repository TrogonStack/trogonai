pub(crate) const MAX_BYTES: usize = 1_048_576;
pub(crate) const MAX_OBJECT_ID_BYTES: usize = 200;
pub(crate) const MAX_CONNECTION_ID_BYTES: usize = MAX_OBJECT_ID_BYTES - "/4294967295/4294967295/32767".len();
