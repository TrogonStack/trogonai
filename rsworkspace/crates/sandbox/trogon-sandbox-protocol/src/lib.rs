//! Sandbox protocol behavior built on the validated contract types: subject and key builders, dispatch
//! authentication, attempt ledgers, policy evaluation, redaction, and sequencing.

mod activity_signal;
mod attempt_ledger;
mod attempt_ledger_error;
mod attempt_ledger_storage;
mod attempt_phase;
mod authority_config;
mod authority_guard;
mod authority_host;
mod authority_host_error;
mod authority_mode;
mod authority_mode_error;
mod authority_oracle;
mod authority_resource_id;
mod authority_resource_id_error;
mod authority_stats;
mod authority_task_completion;
mod authority_task_scope;
mod aws_credential_pair;
mod capped_output;
mod capture_mask_error;
mod classified;
mod classify_subject;
mod cleanup_state;
mod command_exit;
mod command_exit_error;
mod command_reply_fit;
mod compiled_fs_entry;
mod constants;
mod context_verified_subscriber;
mod conventional_aws_pair;
mod conventional_aws_pairing;
mod credential_references;
mod deny_reason_lookup;
mod destination_matching;
mod detached_exit;
mod dispatch_auth_error;
mod dispatch_decode_error;
mod dispatch_envelope;
mod dispatch_fence;
mod dispatch_fence_location;
mod dispatch_global;
mod dispatch_malformed_error;
mod dispatch_public_key_error;
mod dispatch_sign_error;
mod dispatch_signer;
mod dispatch_signer_config_error;
mod dispatch_verb_signing;
mod dispatch_verifier;
mod dispatch_verifier_config_error;
mod dispatch_verifier_verb;
mod domain_entry_messages;
mod domain_pattern_construction;
mod enforcement_mode;
mod enforcement_mode_error;
mod entity_event;
mod entity_transition;
mod environment_digest;
mod environment_digest_error;
mod environment_error;
mod environment_policy;
mod epoch_mode;
mod epoch_mode_error;
mod error_class;
mod error_kind;
mod exec_cwd;
mod extract_error;
mod extract_kind;
mod extract_masking;
mod file_not_found_error;
mod file_op_class;
mod file_op_reply;
mod freeze_outcome;
mod fresh_launch_fence_error;
mod fs_access;
mod fs_policy_evaluator;
mod glob_cursor;
mod glob_cursor_walk;
mod glob_matcher;
mod glob_syntax;
mod glob_token;
mod glob_units;
mod group_one_undefined_error;
mod guest_capture_error;
mod guest_exec;
mod handoff_error;
mod heartbeat;
mod held_lease;
mod host_match;
mod illegal_transition_error;
mod inject_scope_matching;
mod js_regex;
mod js_regex_error;
mod json_scan;
mod jwt_defect;
mod jwt_mask;
mod jwt_mask_error;
mod jwt_structure;
mod kv_key;
mod lease_epoch_assignment;
mod lease_error_subject;
mod ledger_authority_host;
mod lifecycle_emitter;
mod lifecycle_subject;
mod literal_path_ext;
mod loopback;
mod mach_service_match;
mod mask_plan;
mod masked_claim;
mod masked_jwt;
mod native;
mod native_options_error;
mod nats_authority_oracle;
pub mod nats_connect;
mod nats_connect_error;
pub mod nats_transfer;
mod nats_transfer_error;
mod network_options_matching;
mod noop_lifecycle_emitter;
mod operation_request_kind;
mod oracle_error;
mod output_cap;
mod output_join;
mod phase_timer;
mod platform_matching;
mod privileged_unsupported_error;
mod provider;
mod provider_error;
mod provider_phases;
mod proxy_routing;
mod purge_outcome;
mod read_file_error;
mod read_file_response;
mod real_value;
mod redaction_set;
mod redispatch_guard;
mod registry_entry;
mod registry_handoff;
mod reject_unsupported_privileged;
mod request_rule_matching;
mod request_site;
mod request_summary;
mod response_header_matching;
mod restore_verdict;
mod run_phase;
mod sandbox_phase_label;
mod sandbox_users;
mod sentinel;
mod sentinel_registry;
mod sentinel_registry_captures;
mod sentinel_registry_jwt;
mod signed_subject;
mod snapshot_error;
mod snapshot_repository;
mod standard_deny_reason;
mod stdin_admit;
mod stdin_sequencer;
mod stdin_write;
mod stream_redactor;
mod strip_rule;
pub mod subjects;
mod subscribe_verified;
mod substrate_size_class_selection;
mod terminal_frames;
mod terminal_limits;
mod terminal_replay_ring;
mod thaw_outcome;
mod tls_trust;
mod unknown_provider_kind_error;
mod unpaired_reason;
mod verb_scope;
mod verified_request;
mod verified_subscriber;
mod verified_verb_scope;

pub use activity_signal::ActivitySignal;
pub use attempt_ledger::AttemptLedger;
pub use attempt_ledger_error::AttemptLedgerError;
pub use attempt_phase::AttemptPhase;
pub use authority_config::AuthorityConfig;
pub use authority_guard::AuthorityGuard;
pub use authority_host::AuthorityHost;
pub use authority_host_error::AuthorityHostError;
pub use authority_mode::AuthorityMode;
pub use authority_mode_error::AuthorityModeError;
pub use authority_oracle::AuthorityOracle;
pub use authority_resource_id::AuthorityResourceId;
pub use authority_resource_id_error::AuthorityResourceIdError;
pub use authority_stats::AuthorityStats;
pub use authority_task_completion::AuthorityTaskCompletion;
pub use authority_task_scope::AuthorityTaskScope;
pub use aws_credential_pair::AwsCredentialPair;
pub use capped_output::CappedOutput;
pub use capture_mask_error::CaptureMaskError;
pub use classified::Classified;
pub use classify_subject::classify_subject;
pub use cleanup_state::CleanupState;
pub use command_exit::{
    DETACHED_EXIT_POLL_FAILURES, DETACHED_EXIT_POLL_INTERVAL, await_detached_exit, publish_command_exit,
};
pub use command_exit_error::CommandExitError;
pub use command_reply_fit::{fit_poll_command_response, fit_run_command_response};
pub use compiled_fs_entry::CompiledFsEntry;
pub use constants::{
    AUTH_FIELD, AUTH_HEADER, ENV_FENCE_EPOCH, ENV_FENCE_FILE, ENV_PUBLIC_KEYS, ENV_PUBLIC_KEYS_FILE, ENV_SIGNING_KEY,
    ENV_SIGNING_KEY_FILE, ENV_SIGNING_KEY_ID, ENV_SIGNING_TTL_SECS, ENV_VERIFY_MODE, OP_LEASE_DISPATCH,
};
pub use constants::{
    COMMAND_REPLY_MARKER_ROOM, CONFIRMATIONS, DEFAULT_CHECK_INTERVAL, DEFAULT_GRACE, DEFAULT_PARTITION_AFTER,
    ENV_WORKER_AUTHORITY, MAX_LEASES_PER_REQUEST, MIN_LEASE_AGE,
};
pub use context_verified_subscriber::ContextVerifiedSubscriber;
pub use conventional_aws_pair::ConventionalAwsPair;
pub use conventional_aws_pairing::{
    CONVENTIONAL_AWS_ACCESS_KEY_ID, CONVENTIONAL_AWS_SECRET_ACCESS_KEY, CONVENTIONAL_AWS_SESSION_TOKEN,
    conventional_aws_pair,
};
pub use credential_references::{COMMITTED_POLICY_BUCKET, credential_references};
pub use deny_reason_lookup::{denied_domain_reason, deny_list_reason, explain_denied_destination};
pub use destination_matching::{
    denied_domain_matches, destination_is_covered_by, destination_matches, pattern_is_covered_by, pattern_matches,
};
pub use detached_exit::DetachedExit;
pub use dispatch_auth_error::DispatchAuthError;
pub use dispatch_decode_error::DispatchDecodeError;
pub use dispatch_fence_location::{register_fence_file, register_state_dir};
pub use dispatch_global::{check_reassembled, decode_verified, global_verifier};
pub use dispatch_malformed_error::DispatchMalformedError;
pub use dispatch_public_key_error::DispatchPublicKeyError;
pub use dispatch_sign_error::DispatchSignError;
pub use dispatch_signer::DispatchSigner;
pub use dispatch_signer_config_error::DispatchSignerConfigError;
pub use dispatch_verifier::DispatchVerifier;
pub use dispatch_verifier_config_error::DispatchVerifierConfigError;
pub use domain_entry_messages::{ALLOWED_DOMAIN_ENTRY_MESSAGE, DENIED_DOMAIN_ENTRY_MESSAGE};
pub use domain_pattern_construction::{
    exact_denied_domain, exact_pattern, pattern_with_port, pattern_without_port, target_dns_name, target_ip_literal,
};
pub use enforcement_mode::EnforcementMode;
pub use enforcement_mode_error::EnforcementModeError;
pub use entity_event::EntityEvent;
pub use entity_transition::{apply_entity_event, next_entity_state};
pub use environment_digest::{canonical_environment_bytes, environment_digest};
pub use environment_digest_error::EnvironmentDigestError;
pub use environment_error::EnvironmentError;
pub use environment_policy::EnvironmentPolicy;
pub use epoch_mode::EpochMode;
pub use epoch_mode_error::EpochModeError;
pub use error_class::ErrorClass;
pub use error_kind::ErrorKind;
pub use exec_cwd::{join_relative_cwd, resolve_exec_cwd};
pub use extract_error::ExtractError;
pub use extract_kind::ExtractKind;
pub use extract_masking::{mask_extract, plan_extract};
pub use file_not_found_error::FileNotFoundError;
pub use file_op_class::FileOpClass;
pub use file_op_reply::{encode_file_op_response_within, file_op_token, unsupported_file_op_response};
pub use freeze_outcome::FreezeOutcome;
pub use fresh_launch_fence_error::FreshLaunchFenceError;
pub use fs_access::FsAccess;
pub use fs_policy_evaluator::FsPolicyEvaluator;
pub use glob_cursor::GlobCursor;
pub use glob_matcher::GlobMatcher;
pub use glob_syntax::{parse_glob, textual_brackets};
pub use glob_token::GlobToken;
pub use group_one_undefined_error::GroupOneUndefinedError;
pub use guest_capture_error::GuestCaptureError;
pub use guest_exec::{
    GUEST_STREAM_LIMIT, SIGNAL_BY_MARKER_SCRIPT, guest_capture_script, parse_guest_capture,
    retained_guest_capture_cleanup_script, retained_guest_capture_read_script, retained_guest_capture_script,
};
pub use handoff_error::HandoffError;
pub use heartbeat::{DEFAULT_HEARTBEAT_INTERVAL, spawn_heartbeat_loop};
pub use held_lease::HeldLease;
pub use host_match::HostMatch;
pub use illegal_transition_error::IllegalTransitionError;
pub use inject_scope_matching::{
    DEFAULT_INJECT_PORT, inject_effective_pattern, inject_host_matches, inject_omitted_port, inject_path_matches,
    inject_scope_admits, parse_inject_method,
};
pub use js_regex::JsRegex;
pub use js_regex_error::JsRegexError;
pub use jwt_defect::JwtDefect;
pub use jwt_mask::mask_jwt;
pub use jwt_mask_error::JwtMaskError;
pub use jwt_structure::is_jwt;
pub use kv_key::{
    legacy_tenant_kv_key, legacy_tenant_scoped_kv_key, legacy_tenant_scoped_kv_prefix, parse_tenant_scoped_kv_key,
    parse_tenant_scoped_kv_key_any, tenant_kv_key, tenant_scoped_kv_key, tenant_scoped_kv_prefix,
};
pub use lease_epoch_assignment::lease_epoch_for_assignment;
pub use lease_error_subject::LeaseErrorSubject;
pub use ledger_authority_host::LedgerAuthorityHost;
pub use lifecycle_emitter::LifecycleEmitter;
pub use lifecycle_subject::lifecycle_subject;
pub use literal_path_ext::LiteralPathExt;
pub use loopback::{canonical_ip, is_loopback_host, is_loopback_name};
pub use mach_service_match::MachServiceMatch;
pub use mask_plan::MaskPlan;
pub use masked_claim::MaskedClaim;
pub use masked_jwt::MaskedJwt;
pub use native::{
    dispatch_failure_reply, optional_bool, optional_choice, optional_u64, options_for, reject_unknown_keys,
};
pub use native_options_error::NativeOptionsError;
pub use nats_authority_oracle::NatsAuthorityOracle;
pub use nats_connect_error::NatsConnectError;
pub use nats_transfer_error::NatsTransferError;
pub use network_options_matching::{
    DEFAULT_TUNNEL_CAP, domain_only_allows_requires_termination, external_proxy_replaces_runtime_proxy, mitm_routes,
    response_strip, tls_exclusions_exclude, tls_exemptions_name_host, unlisted_destinations_refuse,
};
pub use noop_lifecycle_emitter::NoopLifecycleEmitter;
pub use operation_request_kind::operation_request_kind;
pub use oracle_error::OracleError;
pub use output_cap::{DEFAULT_MAX_OUTPUT_BYTES, cap_buffered, effective_max_output_bytes, recap};
pub use phase_timer::PhaseTimer;
pub use platform_matching::{mach_service_matcher, search_depth_reaches_directory, search_depth_reaches_file};
pub use privileged_unsupported_error::PrivilegedUnsupportedError;
pub use provider::Provider;
pub use provider_error::ProviderError;
pub use provider_phases::phases_supported_by;
pub use proxy_routing::{
    is_parent_proxy_url, no_proxy_bypasses, parent_proxy_bypasses, parent_proxy_route_for, resolve_parent_proxy,
    select_parent_proxy,
};
pub use purge_outcome::PurgeOutcome;
pub use read_file_error::ReadFileError;
pub use read_file_response::{encode_read_file_response_within, read_file_response};
pub use real_value::RealValue;
pub use redaction_set::{MIN_REDACTABLE_LEN, REDACTION_MARKER, RedactionSet};
pub use redispatch_guard::RedispatchGuard;
pub use registry_handoff::RegistryHandoff;
pub use reject_unsupported_privileged::reject_unsupported_privileged;
pub use request_rule_matching::{
    evaluate_request_rules, header_condition_is_met_by, path_prefix_covers, request_rule_matches, request_rule_selects,
};
pub use request_site::RequestSite;
pub use request_summary::RequestSummary;
pub use response_header_matching::{fold_header_name, response_header_is_stripped};
pub use restore_verdict::RestoreVerdict;
pub use run_phase::run_phase;
pub use sandbox_phase_label::sandbox_phase_label;
pub use sandbox_users::{home_dir_for_user, shared_dir_for_group};
pub use sentinel::{SENTINEL_PREFIX, Sentinel};
pub use sentinel_registry::SentinelRegistry;
pub use signed_subject::SignedSubject;
pub use snapshot_error::SnapshotError;
pub use snapshot_repository::{legacy_snapshot_repository_for_tenant, snapshot_repository_for_tenant};
pub use standard_deny_reason::StandardDenyReason;
pub use stdin_admit::StdinAdmit;
pub use stdin_sequencer::StdinSequencer;
pub use stdin_write::StdinWrite;
pub use stream_redactor::StreamRedactor;
pub use strip_rule::StripRule;
pub use subscribe_verified::{
    queue_subscribe_verified, subscribe_verified, subscribe_verified_context, subscribe_with, subscribe_with_context,
};
pub use substrate_size_class_selection::{
    DEFAULT_SUBSTRATE_SIZE_CLASSES, default_substrate_size_classes, max_substrate_memory_mib,
    select_substrate_size_class,
};
pub use terminal_frames::{terminal_data_frame, terminal_input_bytes, terminal_output_bytes, terminal_output_seq};
pub use terminal_limits::{
    MAX_TERMINAL_OUTPUT_CHUNK_BYTES, MAX_TERMINALS_PER_LEASE, TERMINAL_REPLAY_BYTES_ENV,
    terminal_replay_bytes_from_env, terminal_replay_bytes_from_value,
};
pub use terminal_replay_ring::TerminalReplayRing;
pub use thaw_outcome::ThawOutcome;
pub use tls_trust::{CA_TRUST_ENV_VARS, ca_trust_env};
pub use unknown_provider_kind_error::UnknownProviderKindError;
pub use unpaired_reason::UnpairedReason;
pub use verb_scope::VerbScope;
pub use verified_request::VerifiedRequest;
pub use verified_subscriber::VerifiedSubscriber;
pub use verified_verb_scope::VerifiedVerbScope;
