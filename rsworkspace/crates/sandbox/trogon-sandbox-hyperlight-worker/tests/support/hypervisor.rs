//! A hypervisor that records what the worker asks of it and fails on demand.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use trogon_sandbox_hyperlight_worker::{Hypervisor, HypervisorError};

#[derive(Default)]
struct Knobs {
    unavailable: AtomicBool,
    fail_build: AtomicBool,
    fail_load: AtomicBool,
    fail_save: AtomicBool,
    fail_unload: AtomicBool,
    panic_on_load: AtomicBool,
    load_delay_ms: AtomicU64,
    builds: AtomicUsize,
    loads: AtomicUsize,
    unloads: AtomicUsize,
    guests_dropped: AtomicUsize,
}

/// A fake backend; clones share one set of knobs and counters.
#[derive(Clone, Default)]
pub struct FakeHypervisor {
    knobs: Arc<Knobs>,
}

pub struct FakeWarm;

pub struct FakeGuest {
    pub source: String,
    knobs: Arc<Knobs>,
}

impl Drop for FakeGuest {
    fn drop(&mut self) {
        self.knobs.guests_dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl FakeHypervisor {
    pub fn unavailable(&self) {
        self.knobs.unavailable.store(true, Ordering::SeqCst);
    }

    pub fn fail_build(&self, on: bool) {
        self.knobs.fail_build.store(on, Ordering::SeqCst);
    }

    pub fn fail_load(&self, on: bool) {
        self.knobs.fail_load.store(on, Ordering::SeqCst);
    }

    pub fn fail_save(&self, on: bool) {
        self.knobs.fail_save.store(on, Ordering::SeqCst);
    }

    pub fn fail_unload(&self, on: bool) {
        self.knobs.fail_unload.store(on, Ordering::SeqCst);
    }

    pub fn panic_on_load(&self, on: bool) {
        self.knobs.panic_on_load.store(on, Ordering::SeqCst);
    }

    pub fn delay_load(&self, millis: u64) {
        self.knobs.load_delay_ms.store(millis, Ordering::SeqCst);
    }

    pub fn builds(&self) -> usize {
        self.knobs.builds.load(Ordering::SeqCst)
    }

    pub fn loads(&self) -> usize {
        self.knobs.loads.load(Ordering::SeqCst)
    }

    pub fn unloads(&self) -> usize {
        self.knobs.unloads.load(Ordering::SeqCst)
    }

    pub fn guests_dropped(&self) -> usize {
        self.knobs.guests_dropped.load(Ordering::SeqCst)
    }

    fn guest(&self, source: String) -> Result<FakeGuest, HypervisorError> {
        if self.knobs.panic_on_load.load(Ordering::SeqCst) {
            std::panic::resume_unwind(Box::new("the hypervisor died"));
        }
        std::thread::sleep(std::time::Duration::from_millis(
            self.knobs.load_delay_ms.load(Ordering::SeqCst),
        ));
        if self.knobs.fail_load.load(Ordering::SeqCst) {
            return Err(HypervisorError::failed(std::io::Error::other("load refused")));
        }
        self.knobs.loads.fetch_add(1, Ordering::SeqCst);
        Ok(FakeGuest {
            source,
            knobs: Arc::clone(&self.knobs),
        })
    }
}

impl Hypervisor for FakeHypervisor {
    type Warm = FakeWarm;
    type Guest = FakeGuest;

    fn is_available(&self) -> bool {
        !self.knobs.unavailable.load(Ordering::SeqCst)
    }

    fn build_warm(&self) -> Result<FakeWarm, HypervisorError> {
        if self.knobs.fail_build.load(Ordering::SeqCst) {
            return Err(HypervisorError::failed(std::io::Error::other("build refused")));
        }
        self.knobs.builds.fetch_add(1, Ordering::SeqCst);
        Ok(FakeWarm)
    }

    fn load_module(&self, _warm: FakeWarm, component: &Path) -> Result<FakeGuest, HypervisorError> {
        self.guest(fs::read_to_string(component).map_err(HypervisorError::failed)?)
    }

    fn load_layout(&self, _warm: FakeWarm, layout: &Path) -> Result<FakeGuest, HypervisorError> {
        self.guest(fs::read_to_string(layout.join("state")).map_err(HypervisorError::failed)?)
    }

    fn save_layout(&self, guest: &mut FakeGuest, layout: &Path) -> Result<(), HypervisorError> {
        if self.knobs.fail_save.load(Ordering::SeqCst) {
            return Err(HypervisorError::failed(std::io::Error::other("save refused")));
        }
        fs::create_dir_all(layout).map_err(HypervisorError::failed)?;
        fs::write(layout.join("state"), &guest.source).map_err(HypervisorError::failed)
    }

    fn unload(&self, _guest: FakeGuest) -> Result<FakeWarm, HypervisorError> {
        if self.knobs.fail_unload.load(Ordering::SeqCst) {
            return Err(HypervisorError::failed(std::io::Error::other("unload refused")));
        }
        self.knobs.unloads.fetch_add(1, Ordering::SeqCst);
        Ok(FakeWarm)
    }
}
