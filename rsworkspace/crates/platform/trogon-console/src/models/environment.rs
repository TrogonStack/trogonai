use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentType {
    Cloud,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkingType {
    Unrestricted,
    Restricted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub manager: PackageManager,
    pub spec: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PackageManager {
    Apt,
    Cargo,
    Gem,
    Go,
    Npm,
    Pip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub env_type: EnvironmentType,
    pub networking: NetworkingType,
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateEnvironmentRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "type", default = "default_env_type")]
    pub env_type: EnvironmentType,
    #[serde(default = "default_networking")]
    pub networking: NetworkingType,
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

fn default_env_type() -> EnvironmentType {
    EnvironmentType::Cloud
}

fn default_networking() -> NetworkingType {
    NetworkingType::Unrestricted
}

#[derive(Debug, Deserialize)]
pub struct UpdateEnvironmentRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub env_type: Option<EnvironmentType>,
    pub networking: Option<NetworkingType>,
    pub packages: Option<Vec<Package>>,
    pub metadata: Option<std::collections::HashMap<String, String>>,
}
