#[derive(Clone, Copy, Debug)]
pub enum NativeKvCasStage {
    Publish,
    Acknowledge,
}
