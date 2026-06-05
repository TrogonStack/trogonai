#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(coverage, allow(dead_code))] // coverage build cfg-excludes `serve`, orphaning its private server helpers
pub mod config;
pub mod evaluator;
pub mod grader_agent;
pub mod provider;
pub mod provision;
pub mod ralph_loop;
pub mod server;
pub mod service;
pub mod store;
pub mod types;

pub use config::OutcomesConfig;
pub use evaluator::Evaluator;
pub use provider::{
    AnthropicEvaluationProvider, EvalAuthStyle, EvalLlmConfig, EvaluationProvider,
};
pub use provision::{
    EVALUATIONS_STREAM, RESULTS_BUCKET, RUBRICS_BUCKET,
    provision_results_kv, provision_rubrics_kv, provision_stream,
};
pub use grader_agent::{GraderRequest, GraderResponse, SubAgentEvaluationProvider};
pub use ralph_loop::{LoopIteration, RalphLoop, RalphLoopError, RalphLoopResult, TaskExecutor};
pub use ralph_loop::mock::SequencedTaskExecutor;
pub use service::{EvaluationService, trigger_evaluation};
pub use store::{OutcomesStore, ResultClient, RubricClient};
pub use types::{
    Criterion, CriterionScore, EvaluateTrigger, EvaluationResult, OutcomesError, Rubric,
};

pub use server::serve;
