/// How a boundary connection ended.
#[derive(Debug, PartialEq, Eq)]
pub enum BoundaryExit<T> {
    /// The caller's main function returned.
    Main(T),
    /// The peer closed the transport byte stream.
    TransportClosed,
}
