/// Real values shorter than this are still substituted in requests but are not
/// scrubbed from responses: a handful of bytes occurs in unrelated content, so
/// scrubbing it would corrupt bodies while hiding nothing a guess could not find.
pub(crate) const MIN_SCRUBBED_VALUE_LEN: usize = 8;
