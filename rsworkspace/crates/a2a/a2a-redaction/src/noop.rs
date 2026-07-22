use crate::redactor::Redactor;

pub struct NoopRedactor;

impl Redactor for NoopRedactor {}

#[cfg(test)]
mod tests;
