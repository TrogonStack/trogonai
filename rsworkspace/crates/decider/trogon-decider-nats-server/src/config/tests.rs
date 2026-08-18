use trogon_std::FixedArgs;
use trogon_std::env::InMemoryEnv;

use super::*;

fn args() -> Args {
    Args {
        subject_prefix: None,
        queue_group: None,
        events_stream: None,
        snapshot_bucket: None,
        module_store: None,
        modules: Vec::new(),
        admission_limit: None,
        replay_limit: None,
        duplicate_window_secs: None,
    }
}

fn with_module(mut args: Args) -> Args {
    args.modules = vec!["scheduler.schedules@0.1.0".to_owned()];
    args
}

fn reference(value: &str) -> ModuleReference {
    value.parse().expect("a well-formed reference")
}

fn config(args: Args, env: &InMemoryEnv) -> Result<ServerConfig, ConfigError> {
    base_config(&FixedArgs(args), env).map(|(config, _)| config)
}

#[test]
fn the_defaults_are_a_complete_deployment_but_for_the_modules() {
    let config = config(with_module(args()), &InMemoryEnv::new()).expect("defaults resolve");

    assert_eq!(config.endpoint.prefix().as_str(), "decider");
    assert_eq!(config.queue_group, "q");
    assert_eq!(config.events_stream, "DECIDER_EVENTS");
    assert_eq!(config.snapshot_bucket, "DECIDER_SNAPSHOTS");
    assert_eq!(
        config.module_store,
        ModuleStore::ObjectStore("DECIDER_MODULES".to_owned())
    );
    assert_eq!(config.admission_limit.as_usize(), 32);
    assert_eq!(config.replay_limit, None);
    assert_eq!(config.duplicate_window.as_duration(), Duration::from_secs(120));
}

#[test]
fn a_duplicate_window_the_server_would_ignore_stops_startup() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_DUPLICATE_WINDOW_SECS, "0");

    let error = config(with_module(args()), &env).expect_err("a zero window suppresses no duplicate");

    assert!(
        matches!(error, ConfigError::DuplicateWindow { .. }),
        "a command id buys idempotency only for as long as the window remembers its event ids: {error}"
    );
}

#[test]
fn a_host_with_no_module_refuses_to_start() {
    let error = config(args(), &InMemoryEnv::new()).expect_err("a host with no module answers nothing");

    assert!(
        matches!(error, ConfigError::NoModules),
        "starting and then failing every command would look like an outage rather than a misconfiguration: {error}"
    );
}

#[test]
fn modules_come_from_the_environment_as_a_comma_separated_list() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_MODULES, "scheduler.schedules@0.1.0, billing.invoices@2.3.1");

    let config = config(args(), &env).expect("env-configured modules resolve");

    assert_eq!(
        config.modules,
        vec![
            reference("scheduler.schedules@0.1.0"),
            reference("billing.invoices@2.3.1")
        ]
    );
}

#[test]
fn a_module_named_by_a_path_is_no_longer_a_module() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_MODULES, "/modules/schedules.wasm");

    let error = config(args(), &env).expect_err("ADR#0058 names modules by reference, never by path");

    assert!(
        matches!(error, ConfigError::ModuleReference(_)),
        "a deployment carried forward from the path-configured host has to be told it changed: {error}"
    );
}

#[test]
fn the_module_store_is_written_with_the_scheme_that_picks_it() {
    let env = InMemoryEnv::new();

    env.set(ENV_DECIDER_MODULE_STORE, "objectstore:TENANT_MODULES");
    let bucket = config(with_module(args()), &env).expect("a bucket resolves");
    assert_eq!(
        bucket.module_store,
        ModuleStore::ObjectStore("TENANT_MODULES".to_owned())
    );

    env.set(ENV_DECIDER_MODULE_STORE, "file:/srv/modules");
    let directory = config(with_module(args()), &env).expect("a directory resolves");
    assert_eq!(
        directory.module_store,
        ModuleStore::Directory(PathBuf::from("/srv/modules"))
    );
}

#[test]
fn a_store_without_a_scheme_stops_startup() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_MODULE_STORE, "TENANT_MODULES");

    let error = config(with_module(args()), &env).expect_err("a bare name could be either store");

    assert!(
        matches!(error, ConfigError::ModuleStoreScheme { .. }),
        "guessing between a bucket and a relative directory would let a typo change which store is searched: {error}"
    );
}

#[test]
fn a_bucket_name_jetstream_would_reject_stops_startup() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_MODULE_STORE, "objectstore:tenant.modules");

    let error = config(with_module(args()), &env).expect_err("a dot is not a bucket-name character");

    assert!(matches!(error, ConfigError::ModuleBucketName { .. }), "{error}");
}

#[test]
fn a_scheme_no_store_answers_to_stops_startup() {
    let env = InMemoryEnv::new();

    for value in ["s3:tenant-modules", "https://modules.example", "file:"] {
        env.set(ENV_DECIDER_MODULE_STORE, value);

        let error = config(with_module(args()), &env).expect_err("no store answers to that scheme");

        assert!(
            matches!(error, ConfigError::ModuleStoreScheme { .. }),
            "'{value}' names no store this host can search, and falling back to one would search somewhere the operator never asked for: {error}"
        );
    }
}

#[test]
fn a_module_store_reads_back_as_the_store_it_names() {
    let env = InMemoryEnv::new();

    for written in ["objectstore:TENANT_MODULES", "file:/srv/modules"] {
        env.set(ENV_DECIDER_MODULE_STORE, written);
        let store = config(with_module(args()), &env)
            .expect("both schemes resolve")
            .module_store;

        assert_eq!(
            store.to_string(),
            written,
            "a store an operator cannot read back out of a log is one they cannot confirm they configured"
        );
        assert_eq!(
            store.to_string().parse::<ModuleStore>().expect("its own form parses"),
            store
        );
    }
}

#[test]
fn a_flag_wins_over_the_environment() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_SUBJECT_PREFIX, "from-env");

    let mut args = with_module(args());
    args.subject_prefix = Some("from-flag".to_owned());

    let config = config(args, &env).expect("a flag resolves");

    assert_eq!(config.endpoint.prefix().as_str(), "from-flag");
}

#[test]
fn the_prefix_decides_the_subject_the_host_will_answer_on() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_SUBJECT_PREFIX, "acme.decider");

    let config = config(with_module(args()), &env).expect("a dotted prefix resolves");

    assert_eq!(config.endpoint.subject(), "acme.decider.DeciderService.Decide");
}

#[test]
fn a_prefix_that_is_not_a_subject_token_stops_startup() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_SUBJECT_PREFIX, "not a token");

    let error = config(with_module(args()), &env).expect_err("a prefix with a space cannot be subscribed");

    assert!(matches!(error, ConfigError::SubjectPrefix { .. }), "{error}");
}

#[test]
fn an_admission_limit_of_zero_stops_startup() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_ADMISSION_LIMIT, "0");

    let error = config(with_module(args()), &env).expect_err("a host that admits nothing is not a host");

    assert!(matches!(error, ConfigError::AdmissionLimitZero), "{error}");
}

#[test]
fn an_unreadable_admission_limit_stops_startup() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_ADMISSION_LIMIT, "many");

    let error = config(with_module(args()), &env).expect_err("a limit must be a number");

    assert!(
        matches!(error, ConfigError::NotANumber { name, .. } if name == ENV_DECIDER_ADMISSION_LIMIT),
        "falling back to the default would run the host at a concurrency nobody chose: {error}"
    );
}

#[test]
fn a_replay_limit_is_optional_but_never_zero() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_REPLAY_LIMIT, "5000");
    let bounded = config(with_module(args()), &env).expect("a replay limit resolves");
    assert_eq!(bounded.replay_limit.map(ReplayLimit::as_u64), Some(5000));

    env.set(ENV_DECIDER_REPLAY_LIMIT, "0");
    let error = config(with_module(args()), &env).expect_err("zero would fail every command with history");
    assert!(matches!(error, ConfigError::ReplayLimitZero), "{error}");
}

#[test]
fn the_queue_group_and_storage_names_come_from_the_environment() {
    let env = InMemoryEnv::new();
    env.set(ENV_DECIDER_QUEUE_GROUP, "deciders");
    env.set(ENV_DECIDER_EVENTS_STREAM, "TENANT_EVENTS");
    env.set(ENV_DECIDER_SNAPSHOT_BUCKET, "TENANT_SNAPSHOTS");

    let config = config(with_module(args()), &env).expect("storage names resolve");

    assert_eq!(config.queue_group, "deciders");
    assert_eq!(config.events_stream, "TENANT_EVENTS");
    assert_eq!(config.snapshot_bucket, "TENANT_SNAPSHOTS");
}
