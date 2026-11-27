#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExitState {
    Running,
    Exited(Option<i32>),
}
