use std::sync::Mutex;

use super::*;

struct FakeDevice {
    calls: Mutex<Vec<&'static str>>,
    fail: Option<&'static str>,
}

impl FakeDevice {
    fn record(&self, step: &'static str) -> std::io::Result<()> {
        self.calls
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .push(step);
        if self.fail == Some(step) {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
}

impl EntropyDevice for FakeDevice {
    fn add_entropy(&self, _seed: &GuestEntropySeed) -> std::io::Result<()> {
        self.record("add")
    }

    fn reseed_crng(&self) -> std::io::Result<()> {
        self.record("reseed")
    }
}

#[test]
fn reseed_requires_credited_entropy_then_synchronous_crng_reseed() -> Result<(), Box<dyn std::error::Error>> {
    for (fail, expected, success) in [
        (None, vec!["add", "reseed"], true),
        (Some("add"), vec!["add"], false),
        (Some("reseed"), vec!["add", "reseed"], false),
    ] {
        let device = FakeDevice {
            calls: Mutex::default(),
            fail,
        };
        let result = reseed_device(&device, &GuestEntropySeed::new([7; 32]));
        assert_eq!(result.is_ok(), success);
        let calls = device.calls.lock().map_err(|error| error.to_string())?;
        assert_eq!(*calls, expected);
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[test]
fn a_platform_without_the_kernel_interface_refuses() {
    let error = KernelEntropyReseeder.reseed(&GuestEntropySeed::new([1; 32])).err();
    assert_eq!(error.map(|error| error.kind()), Some(std::io::ErrorKind::Unsupported));
}

/// Mixing into the pool needs privilege, which even root lacks inside a container that drops capabilities, so the
/// only honest answers are success or a permission refusal.
#[cfg(target_os = "linux")]
fn accepted_or_refused_for_privilege(result: std::io::Result<()>) -> bool {
    result.map_or_else(|error| error.kind() == std::io::ErrorKind::PermissionDenied, |()| true)
}

#[cfg(target_os = "linux")]
#[test]
fn the_kernel_surfaces_a_refused_reseed() {
    let result = KernelEntropyReseeder.reseed(&GuestEntropySeed::new([1; 32]));
    assert!(accepted_or_refused_for_privilege(result));
}

#[cfg(target_os = "linux")]
#[test]
fn the_crng_reseed_ioctl_answers_by_privilege() -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/urandom")?;
    let result = LinuxEntropyDevice(file).reseed_crng();
    assert!(accepted_or_refused_for_privilege(result));
    Ok(())
}
