use std::time::Duration;

/// Deadline for one readiness probe, kept short so a guest that is still booting does not
/// stall the retry loop.
pub(super) const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// HTTP/1.1 requires a `Host`; nothing on this path resolves it.
pub(super) const HOST: &str = "guest.vsock";

pub(super) const MAX_GUEST_ROOTS: usize = 2 * 1024 * 1024;
pub(super) const MAX_PROXY_CA: usize = 64 * 1024;
pub(super) const MAX_BUNDLE: usize = MAX_GUEST_ROOTS + MAX_PROXY_CA + 2;

pub(super) const INSTALL: &str = r#"set -eu
directory=$1
certificate=$2
shift 2
PATH=/usr/bin:/bin
export PATH
umask 077
mkdir -m 0700 -- "$directory"
cd -P "$directory"
trap 'rm -f ca-bundle.tmp' EXIT HUP INT TERM
found=false
for roots do
    if [ -f "$roots" ] && [ -s "$roots" ]; then
        head -c 2097153 -- "$roots" > ca-bundle.tmp
        size=$(wc -c < ca-bundle.tmp)
        [ "$size" -le 2097152 ] || { echo 'guest public roots exceed 2 MiB' >&2; exit 1; }
        found=true
        break
    fi
done
[ "$found" = true ] || { echo 'guest public root bundle is missing' >&2; exit 1; }
printf '\n%s\n' "$certificate" >> ca-bundle.tmp
chmod 0644 ca-bundle.tmp
mv -f ca-bundle.tmp ca-bundle.pem
chmod 0755 .
"#;
