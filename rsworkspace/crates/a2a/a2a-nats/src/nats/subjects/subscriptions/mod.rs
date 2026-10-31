//! Wildcard subscription subjects (server-side accept filters and JetStream consumer sources).

pub mod agent_all;
pub mod caller_task_events;
pub mod gateway_ingress_all;
pub mod task_all_events;

pub use agent_all::AgentAllSubject;
pub use caller_task_events::CallerTaskEventsSubject;
pub use gateway_ingress_all::GatewayIngressAllSubject;
pub use task_all_events::TaskAllEventsSubject;
