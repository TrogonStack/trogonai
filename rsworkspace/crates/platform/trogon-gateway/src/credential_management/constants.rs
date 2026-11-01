use std::time::Duration;

use super::ManagedCredentialTarget;
use crate::credential::commands::domain::{CredentialKind, SourceKind};

pub(super) const ADMIN_TOKEN_HEADER: &str = "x-trogon-admin-token";
pub(super) const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

pub(super) const IDEMPOTENCY_KEY_PREFIX: &str = "v1.";
pub(super) const CREDENTIAL_MANAGEMENT_IDEMPOTENCY_BUCKET: &str = "GATEWAY_CREDENTIAL_MANAGEMENT_IDEMPOTENCY";
pub(super) const CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
pub(super) const CREDENTIAL_MANAGEMENT_IDEMPOTENCY_IN_PROGRESS_LEASE: Duration = Duration::from_secs(5 * 60);

pub(super) const GITHUB_WEBHOOK_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::GitHub,
    kind: CredentialKind::WebhookSecret,
};

pub(super) const DISCORD_BOT_TOKEN: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Discord,
    kind: CredentialKind::BotToken,
};

pub(super) const GITLAB_SIGNING_TOKEN: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Gitlab,
    kind: CredentialKind::SigningToken,
};

pub(super) const INCIDENTIO_SIGNING_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Incidentio,
    kind: CredentialKind::SigningSecret,
};

pub(super) const LINEAR_WEBHOOK_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Linear,
    kind: CredentialKind::WebhookSecret,
};

pub(super) const MICROSOFT_GRAPH_CLIENT_STATE: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::MicrosoftGraph,
    kind: CredentialKind::ClientState,
};

pub(super) const NOTION_VERIFICATION_TOKEN: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Notion,
    kind: CredentialKind::VerificationToken,
};

pub(super) const PLAID_CLIENT_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Plaid,
    kind: CredentialKind::ClientSecret,
};

pub(super) const SENTRY_CLIENT_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Sentry,
    kind: CredentialKind::ClientSecret,
};

pub(super) const SLACK_SIGNING_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Slack,
    kind: CredentialKind::SigningSecret,
};

pub(super) const TELEGRAM_WEBHOOK_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Telegram,
    kind: CredentialKind::WebhookSecret,
};

pub(super) const TWITTER_CONSUMER_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Twitter,
    kind: CredentialKind::ConsumerSecret,
};
