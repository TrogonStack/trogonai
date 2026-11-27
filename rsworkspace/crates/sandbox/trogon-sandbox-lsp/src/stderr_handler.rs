use std::sync::Arc;

pub(crate) type StderrHandler = Arc<dyn Fn(&str) + Send + Sync>;
