use clap::ValueEnum;

#[derive(Clone, Debug, ValueEnum)]
pub enum IdempotencyBackend {
    InMemory,
    NatsKv,
}
