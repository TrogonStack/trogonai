use crate::Error;

#[derive(Clone)]
pub struct SpiceDbEndpoint(String);
impl SpiceDbEndpoint {
    pub fn new(value: String) -> Result<Self, Error> {
        let url = url::Url::parse(&value).map_err(|_| Error::Configuration)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(Error::Configuration);
        }
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1"));
        if !(url.scheme() == "https" || local && url.scheme() == "http") {
            return Err(Error::Configuration);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
