/// Why a conventional AWS variable was left out of the pair the other two form.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UnpairedReason {
    SecretAccessKeyWithoutAccessKeyId,
    HostsDifferFromAccessKeyId,
}
