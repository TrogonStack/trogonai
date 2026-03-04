//! # trogon-agent
//!
//! Agentic loop that consumes GitHub and Linear events from NATS
//! and calls AI providers through `trogon-secret-proxy` for token-safe requests.
//!
//! ## Architecture
//!
//! ```text
//! GitHub  →  NATS: github.pull_request  →  [runner]  →  pr_review handler
//! Linear  →  NATS: linear.Issue.create  →  [runner]  →  issue_triage handler
//!                                               ↓
//!                                         AgentLoop
//!                                               ↓  (tok_anthropic_prod_xxx)
//!                                    proxy:8080/anthropic/v1/messages
//!                                               ↓  (tok_github_prod_xxx)
//!                                    proxy:8080/github/repos/...
//! ```
//!
//! ## Configuration (env vars)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `NATS_URL` | `localhost:4222` | NATS server URL(s) |
//! | `PROXY_URL` | `http://localhost:8080` | trogon-secret-proxy base URL |
//! | `ANTHROPIC_TOKEN` | — | Opaque token for Anthropic (e.g. `tok_anthropic_prod_xxx`) |
//! | `GITHUB_TOKEN` | — | Opaque token for GitHub API (e.g. `tok_github_prod_xxx`) |
//! | `LINEAR_TOKEN` | — | Opaque token for Linear API (e.g. `tok_linear_prod_xxx`) |
//! | `AGENT_MODEL` | `claude-opus-4-6` | Anthropic model ID |
//! | `AGENT_MAX_ITERATIONS` | `10` | Max loop iterations per event |

pub mod agent_loop;
pub mod config;
pub mod handlers;
pub mod runner;
pub mod tools;

pub use config::AgentConfig;
pub use runner::{RunnerError, run};
