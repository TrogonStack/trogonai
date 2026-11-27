#![allow(dead_code)]

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub mod fake_aca;
pub mod fake_issuer;
pub mod lease;
pub mod local_backends;
pub mod provider;
