use serde::Deserialize;

/// The durable half of a Vercel sandbox: its name and whether it survives a stopped session.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
pub struct SandboxRecord {
    pub name: String,
    #[serde(default)]
    pub persistent: bool,
}
