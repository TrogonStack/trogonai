#[derive(Debug, Clone)]
pub struct CheckpointCreateSpec {
    pub checkpoint_id: String,
    /// Writable directory for checkpoint files (avoids needing root under
    /// `/var/lib/docker/containers/.../checkpoints/`).
    pub checkpoint_dir: String,
}
