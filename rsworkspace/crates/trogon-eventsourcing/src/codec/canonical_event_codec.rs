use super::EventCodec;

pub trait CanonicalEventCodec: Sized {
    type Codec: EventCodec<Self>;

    fn canonical_codec() -> Self::Codec;
}
