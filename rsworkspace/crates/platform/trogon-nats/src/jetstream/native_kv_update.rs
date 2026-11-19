use super::{NativeKvExpectation, NativeKvKey, NativeKvScope};
use bytes::Bytes;
use std::num::NonZeroUsize;

pub struct NativeKvUpdate<'a> {
    pub(super) scope: &'a NativeKvScope,
    pub(super) key: &'a NativeKvKey,
    pub(super) value: Bytes,
    pub(super) expectation: NativeKvExpectation,
    pub(super) maximum_envelope_bytes: NonZeroUsize,
}
impl<'a> NativeKvUpdate<'a> {
    pub fn new(
        scope: &'a NativeKvScope,
        key: &'a NativeKvKey,
        value: Bytes,
        expectation: NativeKvExpectation,
        maximum_envelope_bytes: NonZeroUsize,
    ) -> Self {
        Self {
            scope,
            key,
            value,
            expectation,
            maximum_envelope_bytes,
        }
    }
    pub fn scope(&self) -> &NativeKvScope {
        self.scope
    }
    pub fn key(&self) -> &NativeKvKey {
        self.key
    }
    pub fn value(&self) -> &Bytes {
        &self.value
    }
    pub fn expectation(&self) -> NativeKvExpectation {
        self.expectation
    }
    pub fn maximum_envelope_bytes(&self) -> NonZeroUsize {
        self.maximum_envelope_bytes
    }
}
