use uuid::Uuid;

pub trait NowV7 {
    fn now_v7(&self) -> Uuid;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UuidV7Generator;

impl NowV7 for UuidV7Generator {
    fn now_v7(&self) -> Uuid {
        Uuid::now_v7()
    }
}

#[cfg(test)]
mod tests;
