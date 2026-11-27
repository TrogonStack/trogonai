use trogon_sandbox_contract::PlatformOptions;

/// Refuses options a backend would otherwise ignore. A lease that asks for a
/// relaxation or a capability must get it or fail, never get less silently.
pub fn ensure_enforceable(options: &PlatformOptions, macos: bool, linux: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        macos || options.macos.is_default(),
        "this isolation backend cannot enforce the macOS options of the lease"
    );
    anyhow::ensure!(
        linux || options.linux.is_default(),
        "this isolation backend cannot enforce the Linux options of the lease"
    );
    anyhow::ensure!(
        macos || linux || options.filesystem.is_default(),
        "this isolation backend cannot enforce the filesystem options of the lease"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
