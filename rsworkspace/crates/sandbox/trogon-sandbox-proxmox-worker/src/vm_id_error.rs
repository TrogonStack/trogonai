/// A value is not a Proxmox virtual machine id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VmIdError {
    #[error("VM id {0} is outside Proxmox's range")]
    OutOfRange(u32),
    #[error("a VM id is a decimal number")]
    NotDecimal,
    #[error("a VM id exceeds 32 bits")]
    Overflow,
}
