use clap::ValueEnum;

#[derive(Clone, Debug, ValueEnum)]
pub enum QuotaBackend {
    InMemory,
    NatsKv,
}
