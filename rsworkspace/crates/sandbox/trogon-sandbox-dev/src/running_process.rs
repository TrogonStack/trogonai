use tokio::process::{Child, Command};

use crate::{DevError, ProcessSpec};

/// A spawned process; killed (with its process group on unix) when dropped.
pub struct RunningProcess {
    pub name: &'static str,
    pub child: Child,
}

/// Starts `spec` in its own process group, killed when the returned handle drops.
pub fn spawn(spec: &ProcessSpec) -> Result<RunningProcess, DevError> {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args).envs(&spec.env).kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(target_os = "linux")]
    crate::death_signal::arm(&mut command);
    #[cfg(target_os = "linux")]
    let child = {
        let handle = tokio::runtime::Handle::current();
        crate::spawner::run(move || {
            let _runtime = handle.enter();
            command.spawn()
        })
        .and_then(|spawned| spawned)
    };
    #[cfg(not(target_os = "linux"))]
    let child = command.spawn();
    let child = child.map_err(|source| DevError::Spawn {
        name: spec.name,
        program: spec.program.clone(),
        source,
    })?;
    Ok(RunningProcess { name: spec.name, child })
}

/// Resolves when any process exits, returning its name and exit description.
///
/// With no processes it never resolves.
pub async fn first_exit(running: &mut [RunningProcess]) -> (&'static str, String) {
    if running.is_empty() {
        return std::future::pending().await;
    }
    let waits = running.iter_mut().map(|process| {
        Box::pin(async move {
            let status = process.child.wait().await;
            (process.name, format!("{status:?}"))
        })
    });
    futures::future::select_all(waits).await.0
}
