//! File-watcher-driven hot reload of TLS PEM material.
//!
//! The reloader watches the parent directories of the cert and key
//! paths (editors often rename-over the target, which `notify` only
//! reports as a dir-level event). On any relevant change we re-read
//! the PEMs through [`TlsConfig::load_material`] and publish the
//! fresh [`TlsMaterial`] over a `tokio::sync::watch` channel so
//! servers can swap their `Arc<ServerConfig>` without restarting.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::config::{TlsConfig, TlsConfigError, TlsMaterial};

#[derive(Debug, Clone)]
pub enum ReloadEvent {
    Reloaded,
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ReloadError {
    #[error("watcher init: {0}")]
    Watcher(#[from] notify::Error),
    #[error("initial load: {0}")]
    Initial(#[from] TlsConfigError),
    #[error("watch path missing parent: {0}")]
    NoParent(PathBuf),
}

pub struct PemReloader {
    material_rx: watch::Receiver<Arc<TlsMaterial>>,
    event_rx: mpsc::UnboundedReceiver<ReloadEvent>,
    _watcher: RecommendedWatcher,
    handle: JoinHandle<()>,
}

impl std::fmt::Debug for PemReloader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PemReloader").finish_non_exhaustive()
    }
}

impl PemReloader {
    /// Build a reloader for the given [`TlsConfig`]. The first
    /// load is performed eagerly so the caller has material to serve
    /// before any file events arrive.
    pub fn spawn(cfg: TlsConfig) -> Result<Self, ReloadError> {
        let initial = cfg.load_material()?;
        let (material_tx, material_rx) = watch::channel(Arc::new(initial));
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<()>();

        let cert_dir = parent(&cfg.server_cert_path)?;
        let key_dir = parent(&cfg.server_key_path)?;
        let ca_dir = match &cfg.client_ca_bundle_path {
            Some(p) => Some(parent(p)?),
            None => None,
        };

        let cert_name = cfg.server_cert_path.file_name().map(|n| n.to_os_string());
        let key_name = cfg.server_key_path.file_name().map(|n| n.to_os_string());
        let ca_name = cfg
            .client_ca_bundle_path
            .as_ref()
            .and_then(|p| p.file_name().map(|n| n.to_os_string()));
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            let Ok(ev) = res else { return };
            if !matches!(
                ev.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                return;
            }
            // Compare by file name only: macOS reports paths via the
            // `/private` prefix while tempfile hands us `/var/...`.
            let names_eq = |p: &Path, target: &Option<std::ffi::OsString>| match target {
                Some(t) => p.file_name() == Some(t.as_os_str()),
                None => false,
            };
            let relevant = ev
                .paths
                .iter()
                .any(|p| names_eq(p, &cert_name) || names_eq(p, &key_name) || names_eq(p, &ca_name));
            if relevant {
                let _ = raw_tx.send(());
            }
        })?;

        watcher.watch(&cert_dir, RecursiveMode::NonRecursive)?;
        if key_dir != cert_dir {
            watcher.watch(&key_dir, RecursiveMode::NonRecursive)?;
        }
        if let Some(d) = ca_dir.as_ref()
            && d != &cert_dir
            && d != &key_dir
        {
            watcher.watch(d, RecursiveMode::NonRecursive)?;
        }

        let cfg_for_task = cfg.clone();
        let handle = tokio::spawn(async move {
            // Coalesce bursts: editors often emit several events for a
            // single save. 50ms is enough to let a `mv tmp -> target`
            // settle without being user-visible.
            while raw_rx.recv().await.is_some() {
                while tokio::time::timeout(Duration::from_millis(50), raw_rx.recv())
                    .await
                    .is_ok()
                {}
                match cfg_for_task.load_material() {
                    Ok(m) => {
                        let _ = material_tx.send(Arc::new(m));
                        let _ = event_tx.send(ReloadEvent::Reloaded);
                    }
                    Err(e) => {
                        let _ = event_tx.send(ReloadEvent::Failed(e.to_string()));
                    }
                }
            }
        });

        Ok(Self {
            material_rx,
            event_rx,
            _watcher: watcher,
            handle,
        })
    }

    /// Snapshot of the current material (cheap; bumps the rc).
    pub fn current(&self) -> Arc<TlsMaterial> {
        self.material_rx.borrow().clone()
    }

    /// Subscribe for further updates. Each receiver sees every new
    /// material published after subscription.
    pub fn subscribe(&self) -> watch::Receiver<Arc<TlsMaterial>> {
        self.material_rx.clone()
    }

    /// Await the next reload outcome (success or failure). Returns
    /// `None` once the reloader task ends.
    pub async fn next_event(&mut self) -> Option<ReloadEvent> {
        self.event_rx.recv().await
    }
}

impl Drop for PemReloader {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn parent(p: &Path) -> Result<PathBuf, ReloadError> {
    p.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| ReloadError::NoParent(p.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair};
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::time::timeout;

    fn write_pems(dir: &Path, dns: &str) -> (PathBuf, PathBuf) {
        let kp = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec![dns.into()])
            .unwrap()
            .self_signed(&kp)
            .unwrap();
        let cert_path = dir.join("server.pem");
        let key_path = dir.join("server.key.pem");
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, kp.serialize_pem()).unwrap();
        (cert_path, key_path)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reloads_after_file_rewrite() {
        let dir = tempdir().unwrap();
        let (cert, key) = write_pems(dir.path(), "first.example.com");
        let cfg = TlsConfig {
            server_cert_path: cert.clone(),
            server_key_path: key.clone(),
            client_ca_bundle_path: None,
            spiffe_enabled: false,
            trust_domains: vec![],
            allowed_dns_patterns: vec![],
        };
        let mut reloader = PemReloader::spawn(cfg).expect("reloader");
        let original = reloader.current();
        assert_eq!(original.server_chain.len(), 1);

        // macOS FSEvents needs a moment to register the watcher after
        // spawn before it will deliver subsequent edits.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Rewrite both PEMs with a new identity. Loop until the
        // watcher notices — FSEvents can coalesce or drop the first
        // emission right after watch registration.
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let ev = loop {
            let _ = write_pems(dir.path(), "second.example.com");
            match timeout(Duration::from_millis(500), reloader.next_event()).await {
                Ok(Some(ev)) => break ev,
                Ok(None) => panic!("event sender closed"),
                Err(_) => {
                    if std::time::Instant::now() >= deadline {
                        panic!("no reload event within deadline");
                    }
                }
            }
        };
        assert!(matches!(ev, ReloadEvent::Reloaded), "got {ev:?}");

        let updated = reloader.current();
        assert_ne!(
            original.server_chain[0].as_ref(),
            updated.server_chain[0].as_ref(),
            "material should have changed after rewrite"
        );
    }

    #[tokio::test]
    async fn initial_load_failure_surfaces() {
        let cfg = TlsConfig {
            server_cert_path: PathBuf::from("/nonexistent.pem"),
            server_key_path: PathBuf::from("/nonexistent.key.pem"),
            client_ca_bundle_path: None,
            spiffe_enabled: false,
            trust_domains: vec![],
            allowed_dns_patterns: vec![],
        };
        let err = PemReloader::spawn(cfg).unwrap_err();
        assert!(matches!(err, ReloadError::Initial(_)));
    }
}
