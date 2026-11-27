use std::sync::Arc;

use axum::Router;
use axum::middleware::from_fn_with_state;
use axum::routing::{any, get, post};

use crate::handlers::{
    bind, exec, exec_kill, exec_poll, exec_write_stdin, files_read, files_write, group_membership, groups_create,
    identity, proxy, readyz, terminal_op, terminal_open, users_create,
};
use crate::protocol_admission::admit_protocol;
use crate::{
    AgentState, BIND_PATH, EXEC_KILL_PATH, EXEC_PATH, EXEC_POLL_PATH, EXEC_WRITE_STDIN_PATH, FILES_READ_PATH,
    FILES_WRITE_PATH, GROUP_MEMBERSHIP_PATH, GROUPS_CREATE_PATH, IDENTITY_PATH, READYZ_PATH, TERMINAL_OP_PATH,
    TERMINAL_OPEN_PATH, USERS_CREATE_PATH,
};

/// The agent's HTTP surface: its own routes, with everything else reverse-proxied to the lease's exposed port.
pub fn router(state: Arc<AgentState>) -> Router {
    Router::new()
        .route(READYZ_PATH, get(readyz))
        .route(BIND_PATH, post(bind))
        .route(EXEC_PATH, post(exec))
        .route(EXEC_POLL_PATH, post(exec_poll))
        .route(EXEC_KILL_PATH, post(exec_kill))
        .route(EXEC_WRITE_STDIN_PATH, post(exec_write_stdin))
        .route(FILES_WRITE_PATH, post(files_write))
        .route(FILES_READ_PATH, post(files_read))
        .route(TERMINAL_OPEN_PATH, post(terminal_open))
        .route(TERMINAL_OP_PATH, post(terminal_op))
        .route(USERS_CREATE_PATH, post(users_create))
        .route(GROUPS_CREATE_PATH, post(groups_create))
        .route(GROUP_MEMBERSHIP_PATH, post(group_membership))
        .route(IDENTITY_PATH, get(identity))
        .fallback(any(proxy))
        .layer(from_fn_with_state(Arc::clone(&state), admit_protocol))
        .with_state(state)
}
