use crate::{ConnectionId, Error, ObjectId};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigInput {
    pub user: String,
    pub client: String,
    pub spicedb_endpoint: String,
    pub spicedb_token_env: String,
    pub connections: Vec<ConnectionInput>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionInput {
    pub name: String,
    pub id: String,
    pub dsn_env: String,
}
pub struct RegisteredConnection {
    pub(crate) id: ConnectionId,
    pub(crate) options: crate::DatabaseOptions,
}
pub struct RuntimeConfig {
    pub(crate) user: ObjectId,
    pub(crate) client: ObjectId,
    pub(crate) connections: BTreeMap<String, RegisteredConnection>,
    pub(crate) spicedb_endpoint: crate::SpiceDbEndpoint,
    pub(crate) spicedb_token: String,
}
impl RuntimeConfig {
    pub fn from_input(input: ConfigInput, env: impl Fn(&str) -> Option<String>) -> Result<Self, Error> {
        let mut connections = BTreeMap::new();
        let mut ids = std::collections::BTreeSet::new();
        for connection in input.connections {
            let id = ConnectionId::new(connection.id)?;
            if !ids.insert(id.clone()) {
                return Err(Error::Configuration);
            }
            let name = ObjectId::new(connection.name)?;
            let dsn = env(crate::EnvironmentVariable::new(connection.dsn_env)?.as_str())
                .filter(|v| !v.is_empty())
                .ok_or(Error::Configuration)?;
            if connections
                .insert(
                    name.as_str().to_owned(),
                    RegisteredConnection {
                        id,
                        options: crate::DatabaseOptions::new(&dsn)?,
                    },
                )
                .is_some()
            {
                return Err(Error::Configuration);
            }
        }
        if connections.is_empty() {
            return Err(Error::Configuration);
        }
        Ok(Self {
            user: ObjectId::new(input.user)?,
            client: ObjectId::new(input.client)?,
            connections,
            spicedb_endpoint: crate::SpiceDbEndpoint::new(input.spicedb_endpoint)?,
            spicedb_token: env(crate::EnvironmentVariable::new(input.spicedb_token_env)?.as_str())
                .filter(|v| !v.is_empty())
                .ok_or(Error::Configuration)?,
        })
    }
}
impl RuntimeConfig {
    pub fn spicedb_endpoint(&self) -> &str {
        self.spicedb_endpoint.as_str()
    }
    pub fn spicedb_token(&self) -> &str {
        &self.spicedb_token
    }
}
