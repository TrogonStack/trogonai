/// The scheme a parent proxy is reached with.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ProxyScheme {
    Http,
    Https,
}
