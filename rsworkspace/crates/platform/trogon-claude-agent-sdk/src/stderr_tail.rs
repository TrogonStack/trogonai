/// Diagnostic bytes with content-free debug output; only an empty default is public.
/// This API slice implements no producer, capture, or retention bound.
#[derive(Default)]
pub struct StderrTail(pub(crate) Vec<u8>);

impl StderrTail {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for StderrTail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StderrTail")
            .field("retained_bytes", &self.0.len())
            .finish()
    }
}
