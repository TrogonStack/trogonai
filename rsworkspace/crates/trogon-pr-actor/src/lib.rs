//! # trogon-pr-actor
//!
//! Entity Actor for the full lifecycle of a GitHub pull request.
//!
//! Implements [`EntityActor`][trogon_actor::EntityActor] with persistent
//! [`PrState`][actor::PrState] so every PR review session has access to all
//! prior context:
//!
//! > *Monday: PR #42 opens → actor reviews, notes issues.*
//! > *Wednesday: developer pushes fix → actor loads Monday's state, reviews incrementally.*
//! > *Friday: CI fails → actor correlates failure with the issue it flagged Wednesday.*
//!
//! ## Running
//!
//! The binary (`trogon-pr-actor`) registers with the agent registry and listens
//! on `actors.pr.>`. The `trogon-router` routes matching events here.
//!
//! ## Subject convention
//!
//! Entity key: `{owner}.{repo}.{pr_number}` — NATS-safe (dots not slashes).
//! Full subject: `actors.pr.acme.myrepo.42`

pub mod actor;
