use tracing::warn;

use crate::cli::Cli;

/// Logs every setting that leaves the broker less strict than its defaults.
pub(super) fn log(cli: &Cli) {
    if !cli.enforce_image_allowlist {
        warn!("image allow-list not enforced; any tenant can request any image");
    }
    if cli.allow_no_ttl_leases && cli.default_wall_cap < cli.max_lease_lifetime_seconds {
        warn!(
            default_wall_cap = cli.default_wall_cap,
            max_lease_lifetime_seconds = cli.max_lease_lifetime_seconds,
            "no_ttl leases are charged the lifetime cap, which exceeds the default wall cap; tenants on the \
             default cap cannot hold one"
        );
    }
    if !cli.authz_enforce {
        warn!("broker auth is not enforced; lease requests are unauthenticated");
    }
}
