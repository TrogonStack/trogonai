//! Governed PostgreSQL queries over MCP for a trusted local stdio launcher.
//!
//! Named connections and user/client identities come exclusively from runtime configuration.
//! The bounded SELECT grammar rejects unsupported syntax and catalog features.
//! Every referenced column needs an explicit grant for both identities in one SpiceDB snapshot.
//! Transactional relation locks and cursor metadata bind authorization to the executed relations.
//! Production access and deployment are outside this crate's local implementation contract.
//!
//! The launcher and database administrators are trusted. Callers must not control the configuration
//! or credentials, and the query role must have only the required SELECT privileges.
//! The service installs no database objects and requires no migration coordination. A read-only
//! transaction locks supported base tables, verifies cursor relation identities, and authorizes
//! before fetching any rows. Database administrators, server hooks, and catalog integrity remain
//! trusted; concurrent privileged changes to executable objects, indexes, or shared objects are
//! outside this boundary.
//! Replacing or restoring a database target requires a new connection ID when catalog IDs can repeat.
//! Permissions are checked at admission; revocation does not retroactively cancel an admitted query.

mod config;
mod constants;
mod database_options;
mod environment_variable;
mod error;
mod identifier;
mod object_id;
mod parameter;
mod parser;
mod postgres;
mod row_limit;
mod server;
mod spicedb;
mod spicedb_endpoint;
mod telemetry;

pub use config::{ConfigInput, RegisteredConnection, RuntimeConfig};
pub use database_options::DatabaseOptions;
pub use environment_variable::EnvironmentVariable;
pub use error::Error;
pub use identifier::Identifier;
pub use object_id::ObjectId;
pub use parameter::{Parameter, ParameterInput};
pub use parser::Query;
pub use postgres::{QueryResult, QueryService};
pub use server::PostgresMcp;
pub use spicedb::{PermissionCheck, PermissionRequest, Resource, SpiceDb};
pub use spicedb_endpoint::SpiceDbEndpoint;
