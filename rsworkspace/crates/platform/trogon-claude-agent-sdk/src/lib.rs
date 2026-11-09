#![doc = include_str!("../README.md")]

mod client;
mod error;
mod options;
mod permission_mode;
mod setting_source;
mod usd_budget;

pub use client::ClaudeSdkClient;
pub use error::Error;
pub use options::ClaudeAgentOptions;
pub use permission_mode::PermissionMode;
pub use setting_source::SettingSource;

pub use usd_budget::{UsdBudget, UsdBudgetError};
