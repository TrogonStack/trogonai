#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        acyclic_modules,
        reason = "`bridge` dispatches each ACP method to its handler module and every handler takes the `Bridge` it was dispatched from"
    )
)]

mod archive_session;
mod auth_status;
mod authenticate;
mod bridge;
mod cancel;
mod close_session;
mod delete_session;
mod ext_method;
mod ext_notification;
mod fork_session;
mod initialize;
pub(crate) mod js_request;
mod list_sessions;
mod load_session;
mod logout;
mod new_session;
mod prompt;
mod providers_disable;
mod providers_list;
mod providers_set;
mod resume_session;
pub(crate) mod rpc_call;
#[cfg(test)]
mod schema_1_15_tests;
mod session_status;
mod set_session_config_option;
mod set_session_mode;
mod set_session_title;
#[cfg(test)]
pub(crate) mod test_support;
mod unarchive_session;

pub use bridge::Bridge;

#[cfg(test)]
mod tests;
