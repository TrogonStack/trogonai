use std::fs::File;
use std::io;
use std::os::windows::io::{FromRawHandle, RawHandle};

use crate::args::ControlFd;
mod constants;
use constants::{
    DUPLICATE_SAME_ACCESS, ERROR_ACCESS_DENIED, HANDLE_FLAG_INHERIT, INVALID_HANDLE, NO_ASSOCIATED_STREAM,
};

type Handle = isize;

type InvalidParameterHandler = Option<unsafe extern "C" fn(*const u16, *const u16, *const u16, u32, usize)>;

unsafe extern "C" {
    fn _get_osfhandle(fd: i32) -> Handle;
    fn _set_thread_local_invalid_parameter_handler(handler: InvalidParameterHandler) -> InvalidParameterHandler;
}

unsafe extern "C" fn ignore_invalid_parameter(_: *const u16, _: *const u16, _: *const u16, _: u32, _: usize) {}

fn handle_of(fd: ControlFd) -> Handle {
    unsafe {
        let previous = _set_thread_local_invalid_parameter_handler(Some(ignore_invalid_parameter));
        let handle = _get_osfhandle(fd.raw());
        _set_thread_local_invalid_parameter_handler(previous);
        handle
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inheritable: i32,
        options: u32,
    ) -> i32;
    fn SetHandleInformation(handle: Handle, mask: u32, flags: u32) -> i32;
    fn PeekNamedPipe(
        pipe: Handle,
        buffer: *mut core::ffi::c_void,
        size: u32,
        read: *mut u32,
        available: *mut u32,
        left: *mut u32,
    ) -> i32;
}

/// The handle behind the CRT descriptor number the caller gave, which is how
/// a Windows parent hands one over, as a reader of its own that the command
/// cannot inherit.
pub(super) fn open_reader(fd: ControlFd) -> Result<File, String> {
    let handle = handle_of(fd);
    if handle == INVALID_HANDLE || handle == NO_ASSOCIATED_STREAM {
        return Err("EBADF: bad file descriptor".to_string());
    }
    if unsafe {
        PeekNamedPipe(
            handle,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
        && io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED)
    {
        return Err("EBADF: bad file descriptor, it is open for writing only".to_string());
    }
    let mut private: Handle = 0;
    let process = unsafe { GetCurrentProcess() };
    if unsafe { DuplicateHandle(process, handle, process, &mut private, 0, 0, DUPLICATE_SAME_ACCESS) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let reader = unsafe { File::from_raw_handle(private as RawHandle) };
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(format!(
            "cannot keep it from the command: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(reader)
}
