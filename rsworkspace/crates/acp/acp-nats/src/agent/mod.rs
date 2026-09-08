#![cfg_attr(
    dylint_lib = "trogon_lints",
    expect(
        acyclic_modules,
        reason = "`bridge` dispatches each ACP method to its handler module and every handler takes the `Bridge` it was dispatched from"
    )
)]

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
mod set_session_config_option;
mod set_session_mode;
#[cfg(test)]
pub(crate) mod test_support;

pub use bridge::Bridge;

#[cfg(test)]
mod tests;
