/// Model selection whose string never appears in debug output.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ModelName(String);

impl ModelName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ModelName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("ModelName").finish_non_exhaustive()
    }
}

impl From<String> for ModelName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ModelName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}
