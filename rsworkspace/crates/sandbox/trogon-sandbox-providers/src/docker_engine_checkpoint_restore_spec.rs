#[derive(Debug, Clone)]
pub struct CheckpointRestoreSpec {
    pub checkpoint_id: String,
    pub checkpoint_dir: String,
}
