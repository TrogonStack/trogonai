//! Governed PostgreSQL queries over MCP for a trusted local stdio launcher.
//!
//! Named connections and user/client identities come exclusively from runtime configuration.
//! The bounded SELECT grammar rejects unsupported syntax and catalog features.
//! Every referenced column needs an explicit grant for both identities in one SpiceDB snapshot.
//! The required administrator-installed DDL guard fences catalog changes until the query ends.
//! Production access and deployment are outside this crate's local implementation contract.
//!
//! The launcher and database administrators are trusted. Callers must not control the configuration
//! or credentials, and the query role must have only the required SELECT privileges.
//! Install `sql/install-ddl-guard.sql` in each registered database. Migrations must take the session
//! advisory lock `pg_advisory_lock(1879445091, 1347635021)` before touching relations and retain it
//! through all transactions. This is essential for concurrent index operations, guard maintenance,
//! and shared-object changes that the event trigger cannot fence on its own.
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
