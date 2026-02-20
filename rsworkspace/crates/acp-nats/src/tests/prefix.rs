/// Tests for custom ACP prefix functionality
///
/// The acp_prefix parameter completely replaces "acp" as the root token in all NATS subjects.
/// This allows multi-tenancy, deployment isolation, and running multiple instances.
use crate::nats::{agent, client};

#[test]
fn test_default_prefix_agent_subjects() {
    let prefix = "acp";

    assert_eq!(agent::initialize(prefix), "acp.agent.initialize");
    assert_eq!(agent::authenticate(prefix), "acp.agent.authenticate");
    assert_eq!(agent::session_new(prefix), "acp.agent.session.new");
}

#[test]
fn test_default_prefix_session_subjects() {
    let prefix = "acp";
    let session_id = "sess123";

    assert_eq!(
        agent::session_load(prefix, session_id),
        "acp.sess123.agent.session.load"
    );
    assert_eq!(
        agent::session_prompt(prefix, session_id),
        "acp.sess123.agent.session.prompt"
    );
    assert_eq!(
        agent::session_cancel(prefix, session_id),
        "acp.sess123.agent.session.cancel"
    );
    assert_eq!(
        agent::session_set_mode(prefix, session_id),
        "acp.sess123.agent.session.set_mode"
    );
}

#[test]
fn test_default_prefix_client_subjects() {
    let prefix = "acp";
    let session_id = "sess123";

    assert_eq!(
        client::fs_read_text_file(prefix, session_id),
        "acp.sess123.client.fs.read_text_file"
    );
    assert_eq!(
        client::terminal_create(prefix, session_id),
        "acp.sess123.client.terminal.create"
    );
}

#[test]
fn test_custom_prefix_replaces_acp_token() {
    let prefix = "myapp";

    // Agent subjects
    assert_eq!(agent::initialize(prefix), "myapp.agent.initialize");
    assert_eq!(agent::authenticate(prefix), "myapp.agent.authenticate");
    assert_eq!(agent::session_new(prefix), "myapp.agent.session.new");

    // Session subjects
    let session_id = "sess123";
    assert_eq!(
        agent::session_prompt(prefix, session_id),
        "myapp.sess123.agent.session.prompt"
    );

    // Client subjects
    assert_eq!(
        client::fs_read_text_file(prefix, session_id),
        "myapp.sess123.client.fs.read_text_file"
    );
}

#[test]
fn test_multi_tenant_prefixes() {
    let session_id = "sess123";

    // Tenant 1
    let tenant1 = "tenant1";
    assert_eq!(agent::initialize(tenant1), "tenant1.agent.initialize");
    assert_eq!(
        agent::session_prompt(tenant1, session_id),
        "tenant1.sess123.agent.session.prompt"
    );

    // Tenant 2
    let tenant2 = "tenant2";
    assert_eq!(agent::initialize(tenant2), "tenant2.agent.initialize");
    assert_eq!(
        agent::session_prompt(tenant2, session_id),
        "tenant2.sess123.agent.session.prompt"
    );

    // Tenants are completely isolated
    assert_ne!(agent::initialize(tenant1), agent::initialize(tenant2));
}

#[test]
fn test_environment_based_prefixes() {
    // Development environment
    let dev_prefix = "dev";
    assert_eq!(agent::initialize(dev_prefix), "dev.agent.initialize");

    // Production environment
    let prod_prefix = "prod";
    assert_eq!(agent::initialize(prod_prefix), "prod.agent.initialize");

    // Staging environment
    let staging_prefix = "staging";
    assert_eq!(
        agent::initialize(staging_prefix),
        "staging.agent.initialize"
    );
}

#[test]
fn test_wildcard_pattern_structure() {
    // While we can't access wildcards directly in tests due to module visibility,
    // we can document the expected wildcard patterns here for reference:
    //
    // With prefix "myapp":
    // - All messages: "myapp.>"
    // - All agent messages: "myapp.agent.>"
    // - All sessions: "myapp.*.agent.>"
    // - Specific session: "myapp.sess123.agent.>"
    // - All client messages: "myapp.*.client.>"

    let prefix = "myapp";
    let session_id = "sess123";

    // Verify the base subject patterns align with wildcard patterns
    assert!(agent::initialize(prefix).starts_with("myapp."));
    assert!(agent::session_prompt(prefix, session_id).starts_with("myapp.sess123."));
    assert!(client::fs_read_text_file(prefix, session_id).starts_with("myapp.sess123.client."));
}

#[test]
fn test_extension_subjects_with_custom_prefix() {
    let prefix = "myapp";
    let session_id = "sess123";
    let method = "custom.method";

    // Session-less extension
    assert_eq!(agent::ext(prefix, method), "myapp.agent.ext.custom.method");

    // Session ready extension
    assert_eq!(
        agent::ext_session_ready(prefix, session_id),
        "myapp.sess123.agent.ext.session.ready"
    );

    // Client extension - prompt response
    assert_eq!(
        client::ext_session_prompt_response(prefix, session_id),
        "myapp.sess123.client.ext.session.prompt_response"
    );
}

#[test]
fn test_prefix_does_not_add_acp_namespace() {
    // IMPORTANT: The prefix REPLACES "acp", it doesn't wrap it
    let prefix = "myapp";

    // ✅ Correct: myapp.agent.initialize
    assert_eq!(agent::initialize(prefix), "myapp.agent.initialize");

    // ❌ Incorrect: myapp.acp.agent.initialize (this is NOT the behavior)
    assert_ne!(agent::initialize(prefix), "myapp.acp.agent.initialize");
}

#[test]
fn test_special_characters_in_prefix() {
    // Underscore prefix (common pattern)
    let prefix = "my_app";
    assert_eq!(agent::initialize(prefix), "my_app.agent.initialize");

    // Hyphen prefix
    let prefix = "my-app";
    assert_eq!(agent::initialize(prefix), "my-app.agent.initialize");

    // Numeric prefix
    let prefix = "app123";
    assert_eq!(agent::initialize(prefix), "app123.agent.initialize");
}
