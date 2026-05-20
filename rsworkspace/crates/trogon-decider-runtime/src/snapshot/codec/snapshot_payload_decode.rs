#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct SnapshotPayloadData<'a> {
    pub payload: &'a [u8],
}

impl<'a> SnapshotPayloadData<'a> {
    pub const fn new(payload: &'a [u8]) -> Self {
        Self { payload }
    }
}

pub trait SnapshotPayloadDecode: Sized {
    type Error;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error>;
}
