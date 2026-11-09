#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProducerStatus {
    Active,
    Poisoned,
}
