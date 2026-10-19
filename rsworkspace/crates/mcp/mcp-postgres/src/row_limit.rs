use crate::Error;

#[derive(Debug)]
pub(crate) struct RowLimit {
    requested: Option<usize>,
}
impl RowLimit {
    pub(crate) fn bounded_default() -> Self {
        Self { requested: None }
    }
    pub(crate) fn requested(value: usize) -> Result<Self, Error> {
        if value > 1000 {
            return Err(Error::Limit);
        }
        Ok(Self { requested: Some(value) })
    }
    pub(crate) fn statement_count(&self) -> usize {
        self.requested.unwrap_or(1001)
    }
    pub(crate) fn result_count(&self) -> usize {
        self.requested.unwrap_or(1000)
    }
}
