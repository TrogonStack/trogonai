//! The `Internet Settings` key of the current user, read as `winreg.OpenKey` and `winreg.QueryValueEx` read it, and
//! the lowercasing `ntpath.normcase` does through `_winapi.LCMapStringEx`, failing with the `OSError` it raises.

use super::{InternetSettings, ReadInternetSettings, from_utf16, reg2py, to_utf16, windows_error};
use crate::RemoteSchemaError;
use crate::constants::{
    DEFAULT_LANGUAGE, ERROR_MORE_DATA, ERROR_SUCCESS, FORMAT_MESSAGE_FLAGS, HKEY_CURRENT_USER, INTERNET_SETTINGS,
    KEY_READ, LCMAP_LOWERCASE,
};
use crate::python_string::PythonString;
use std::ffi::c_void;

type Hkey = *mut c_void;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegOpenKeyExW(key: Hkey, sub_key: *const u16, options: u32, access: u32, result: *mut Hkey) -> i32;
    fn RegQueryValueExW(
        key: Hkey,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: Hkey) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LCMapStringEx(
        locale: *const u16,
        flags: u32,
        source: *const u16,
        source_size: i32,
        destination: *mut u16,
        destination_size: i32,
        version: *mut c_void,
        reserved: *mut c_void,
        sort_handle: isize,
    ) -> i32;
    fn GetLastError() -> u32;
    fn FormatMessageW(
        flags: u32,
        source: *const c_void,
        message_id: u32,
        language_id: u32,
        buffer: *mut u16,
        size: u32,
        arguments: *mut c_void,
    ) -> u32;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

/// `PyErr_SetFromWindowsErr(GetLastError())`: the `OSError` for the last error of the thread, with the message the
/// system holds for it.
pub(crate) fn last_error() -> RemoteSchemaError {
    // SAFETY: the call only reads the last error of the calling thread.
    let code = unsafe { GetLastError() };
    let mut buffer: *mut u16 = std::ptr::null_mut();
    // SAFETY: with `FORMAT_MESSAGE_ALLOCATE_BUFFER` the call allocates the message and writes its address to `buffer`,
    // which it is given in place of the buffer itself.
    let length = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FLAGS,
            std::ptr::null(),
            code,
            DEFAULT_LANGUAGE,
            (&raw mut buffer).cast(),
            0,
            std::ptr::null_mut(),
        )
    };
    let message = usize::try_from(length)
        .ok()
        .filter(|&length| length > 0 && !buffer.is_null())
        .map(|length| {
            // SAFETY: `buffer` holds the `length` code units the call wrote.
            unsafe { std::slice::from_raw_parts(buffer, length) }.to_vec()
        });
    if !buffer.is_null() {
        // SAFETY: `buffer` was allocated by `FormatMessageW` and is freed once, after its last use.
        unsafe { LocalFree(buffer.cast()) };
    }
    windows_error(code, message.as_deref())
}

/// The text as a NUL-terminated UTF-16 string.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// `winreg.QueryValueEx(key, name)[0]`, `None` for the `OSError` it raises. The data is converted at the size of the
/// buffer, as `Reg2Py(retBuf, bufSize, typ)` converts it: when the second query writes less, the rest of the buffer
/// is zero here and uninitialized in C.
///
/// # Safety
///
/// `key` is an open registry key.
unsafe fn query_value(key: Hkey, name: &str) -> Option<super::RegistryValue> {
    let name = wide(name);
    let mut size: u32 = 0;
    // SAFETY: `key` is open, `name` is NUL-terminated, and a null buffer asks only for the size.
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    };
    match status {
        ERROR_SUCCESS => {}
        ERROR_MORE_DATA => size = 256,
        _ => return None,
    }
    let mut data = vec![0u8; usize::try_from(size).ok()?];
    let mut kind: u32 = 0;
    loop {
        let mut written = size;
        // SAFETY: `data` holds `size` bytes, the size the call is told it may write.
        let status = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null_mut(),
                &raw mut kind,
                data.as_mut_ptr(),
                &raw mut written,
            )
        };
        match status {
            ERROR_SUCCESS => return Some(reg2py(&data, kind)),
            ERROR_MORE_DATA => {
                size = size.checked_mul(2)?;
                data.resize(usize::try_from(size).ok()?, 0);
            }
            _ => return None,
        }
    }
}

/// The `Internet Settings` key of the current user.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InternetSettingsKey;

impl ReadInternetSettings for InternetSettingsKey {
    fn read(&self) -> InternetSettings {
        let sub_key = wide(INTERNET_SETTINGS);
        let mut key: Hkey = std::ptr::null_mut();
        // SAFETY: the predefined key needs no opening, `sub_key` is NUL-terminated, and the opened key is written to
        // `key`.
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER as Hkey, sub_key.as_ptr(), 0, KEY_READ, &raw mut key) };
        if status != ERROR_SUCCESS {
            return InternetSettings::default();
        }
        // SAFETY: `key` was opened above and stays open until it is closed below.
        let settings = unsafe {
            InternetSettings {
                proxy_enable: query_value(key, "ProxyEnable"),
                proxy_server: query_value(key, "ProxyServer"),
                proxy_override: query_value(key, "ProxyOverride"),
            }
        };
        // SAFETY: `key` was opened above and is closed once, after its last use.
        unsafe { RegCloseKey(key) };
        settings
    }

    /// `_winapi.LCMapStringEx("", LCMAP_LOWERCASE, text)`, the invariant locale being the empty name: `OverflowError`
    /// for a text longer than an `int` counts, and the `OSError` of the last error where either call fails.
    fn lowercase(&self, text: &PythonString) -> Result<PythonString, RemoteSchemaError> {
        let source = to_utf16(text);
        let Ok(source_size) = i32::try_from(source.len()) else {
            return Err(RemoteSchemaError::new("OverflowError", "input string is too long"));
        };
        let locale = wide("");
        // SAFETY: `source` holds `source_size` code units, and a null destination of size 0 asks only for the size.
        let size = unsafe {
            LCMapStringEx(
                locale.as_ptr(),
                LCMAP_LOWERCASE,
                source.as_ptr(),
                source_size,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        let Some(capacity) = usize::try_from(size).ok().filter(|&capacity| capacity > 0) else {
            return Err(last_error());
        };
        let mut destination = vec![0u16; capacity];
        // SAFETY: `destination` holds `size` code units, the size the call is told it may write.
        let written = unsafe {
            LCMapStringEx(
                locale.as_ptr(),
                LCMAP_LOWERCASE,
                source.as_ptr(),
                source_size,
                destination.as_mut_ptr(),
                size,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        match usize::try_from(written).ok().filter(|&written| written > 0) {
            Some(written) => Ok(from_utf16(destination.get(..written).unwrap_or_default())),
            None => Err(last_error()),
        }
    }
}
