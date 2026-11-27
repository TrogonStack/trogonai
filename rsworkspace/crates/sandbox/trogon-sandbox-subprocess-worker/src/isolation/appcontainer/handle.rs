use windows::Win32::Foundation::{CloseHandle, HANDLE};

pub struct OwnedHandle(HANDLE);

// A kernel handle is a process-wide table index, usable from any thread.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    pub fn new(handle: HANDLE) -> Self {
        Self(handle)
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}
