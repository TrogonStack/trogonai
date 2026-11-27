use serde::Deserialize;

/// The public URL Vercel assigned to one declared sandbox port.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
pub struct Route {
    pub url: String,
    pub port: u16,
}
