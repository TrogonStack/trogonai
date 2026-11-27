use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::NetworkPolicy;

use crate::snapshot::LocalSnapshot;
use crate::vm_dir::VmDir;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WarmKey([u8; 32]);

/// What makes two warm templates interchangeable.
#[derive(Debug, Serialize)]
struct TemplateIdentity<'a> {
    base: &'a str,
    kernel: &'a Path,
    boot_args: &'a str,
    memory_mib: u32,
    vcpus: u8,
    agent_port: u32,
    guest_cid: u32,
    network: Option<&'a NetworkPolicy>,
}

impl WarmKey {
    pub fn new(
        base: &str,
        config: &crate::config::FirecrackerConfig,
        memory: crate::config::MemoryMib,
        policy: Option<&NetworkPolicy>,
    ) -> Self {
        let identity = TemplateIdentity {
            base,
            kernel: config.kernel.as_path(),
            boot_args: &config.boot_args,
            memory_mib: memory.get(),
            vcpus: config.default_vcpus.get(),
            agent_port: config.agent_port.get(),
            guest_cid: config.guest_cid.get(),
            network: policy,
        };
        let data = serde_json::to_vec(&identity).unwrap_or_else(|_| format!("{identity:?}").into_bytes());
        Self(Sha256::digest(data).into())
    }
}

pub use crate::settings::WarmPoolConfig;

struct Entry {
    snapshot: Arc<LocalSnapshot>,
    bytes: u64,
    last_used: Instant,
}

pub struct WarmPool {
    config: WarmPoolConfig,
    entries: HashMap<WarmKey, Entry>,
}

impl WarmPool {
    pub fn new(config: WarmPoolConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.max_snapshots > 0 && self.config.max_bytes > 0
    }

    pub fn get(&mut self, key: &WarmKey) -> Option<Arc<LocalSnapshot>> {
        self.sweep();
        self.entries.get_mut(key).map(|entry| {
            entry.last_used = Instant::now();
            Arc::clone(&entry.snapshot)
        })
    }

    pub fn sweep(&mut self) {
        self.entries
            .retain(|_, entry| entry.last_used.elapsed() < self.config.max_idle);
    }

    pub async fn insert(&mut self, key: WarmKey, snapshot: LocalSnapshot) -> anyhow::Result<()> {
        self.sweep();
        if !self.enabled() {
            return Ok(());
        }
        if self.entries.contains_key(&key) {
            return Ok(());
        }
        let mut entries = tokio::fs::read_dir(snapshot.path()).await?;
        let mut bytes = 0u64;
        while let Some(entry) = entries.next_entry().await? {
            bytes = bytes.saturating_add(entry.metadata().await?.len());
        }
        if bytes > self.config.max_bytes {
            return Ok(());
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= self.config.max_snapshots
                || self
                    .entries
                    .values()
                    .map(|entry| entry.bytes)
                    .sum::<u64>()
                    .saturating_add(bytes)
                    > self.config.max_bytes)
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            key,
            Entry {
                snapshot: Arc::new(snapshot),
                bytes,
                last_used: Instant::now(),
            },
        );
        Ok(())
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

pub async fn restore(snapshot: &LocalSnapshot, vm: &VmDir) -> anyhow::Result<()> {
    for file in ["rootfs.ext4", crate::snapshot::STATE_FILE, crate::snapshot::MEMORY_FILE] {
        crate::disk::copy_private(&snapshot.path().join(file), &vm.runtime_dir().join(file)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
