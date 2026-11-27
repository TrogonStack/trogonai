#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;

#[cfg(target_os = "linux")]
use crate::syscall_result::syscall_result;
use crate::{EntropyReseeder, GuestEntropySeed};

/// Reseeds the Linux kernel's random pool through the random device.
///
/// Any other platform refuses, because a seed that was not applied must never be reported as applied.
#[derive(Debug, Clone, Copy, Default)]
pub struct KernelEntropyReseeder;

impl EntropyReseeder for KernelEntropyReseeder {
    #[cfg(target_os = "linux")]
    fn reseed(&self, seed: &GuestEntropySeed) -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/urandom")?;
        reseed_device(&LinuxEntropyDevice(file), seed)
    }

    #[cfg(not(target_os = "linux"))]
    fn reseed(&self, _seed: &GuestEntropySeed) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "kernel entropy reseed requires Linux",
        ))
    }
}

#[cfg(any(target_os = "linux", test))]
trait EntropyDevice {
    fn add_entropy(&self, seed: &GuestEntropySeed) -> std::io::Result<()>;
    fn reseed_crng(&self) -> std::io::Result<()>;
}

/// Credits the seed to the pool, then forces the generator to reseed from it before returning.
#[cfg(any(target_os = "linux", test))]
fn reseed_device(device: &impl EntropyDevice, seed: &GuestEntropySeed) -> std::io::Result<()> {
    device.add_entropy(seed)?;
    device.reseed_crng()
}

#[cfg(target_os = "linux")]
struct LinuxEntropyDevice(std::fs::File);

#[cfg(target_os = "linux")]
#[repr(C)]
struct PoolInfo {
    entropy_count: libc::c_int,
    buf_size: libc::c_int,
    bytes: [u8; GuestEntropySeed::LEN],
}

#[cfg(target_os = "linux")]
impl EntropyDevice for LinuxEntropyDevice {
    fn add_entropy(&self, seed: &GuestEntropySeed) -> std::io::Result<()> {
        let info = PoolInfo {
            entropy_count: 256,
            buf_size: 32,
            bytes: *seed.as_bytes(),
        };
        // The random ioctl header is two ints, followed by `buf_size` bytes.
        let command = libc::_IOW::<[libc::c_int; 2]>(u32::from(b'R'), 0x03);
        // SAFETY: `info` is a live, correctly laid out `rand_pool_info` for the whole call.
        let result = unsafe { libc::ioctl(self.0.as_raw_fd(), command, &info) };
        syscall_result(result)
    }

    fn reseed_crng(&self) -> std::io::Result<()> {
        // SAFETY: `RNDRESEEDCRNG` takes no argument.
        let result = unsafe { libc::ioctl(self.0.as_raw_fd(), libc::_IO(u32::from(b'R'), 0x07)) };
        syscall_result(result)
    }
}

#[cfg(test)]
mod tests;
