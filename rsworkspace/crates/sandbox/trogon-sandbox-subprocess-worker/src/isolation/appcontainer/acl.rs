use std::path::Path;

use anyhow::{Context, ensure};
use windows::Win32::Foundation::{HLOCAL, LocalFree, WIN32_ERROR};
use windows::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW,
    SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows::Win32::Storage::FileSystem::{FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};
use windows::core::{PCWSTR, PWSTR};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Full,
}

impl Access {
    fn mask(self) -> u32 {
        match self {
            Self::Read => (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE).0,
            Self::Full => FILE_ALL_ACCESS.0,
        }
    }
}

fn check(code: WIN32_ERROR, call: &str) -> anyhow::Result<()> {
    ensure!(code.is_ok(), "{call} failed with 0x{:08x}", code.0);
    Ok(())
}

struct LocalAllocation(*mut core::ffi::c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0)));
            }
        }
    }
}

/// Adds one allow entry for `sid` to the DACL of `path`, inherited by
/// everything created in the directory later and applied to what is in it
/// now. The other entries stay as they are, so this is only for a directory
/// the worker made for the lease.
pub fn grant(path: &Path, sid: PSID, access: Access) -> anyhow::Result<()> {
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let name = PCWSTR(wide.as_ptr());

    let mut current: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    check(
        unsafe {
            GetNamedSecurityInfoW(
                name,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&mut current),
                None,
                &mut descriptor,
            )
        },
        "GetNamedSecurityInfoW",
    )
    .with_context(|| format!("reading the access list of {}", path.display()))?;
    let _descriptor = LocalAllocation(descriptor.0);

    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access.mask(),
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: PWSTR(sid.0.cast()),
        },
    };
    let mut merged: *mut ACL = std::ptr::null_mut();
    check(
        unsafe { SetEntriesInAclW(Some(&[entry]), Some(current), &mut merged) },
        "SetEntriesInAclW",
    )?;
    let _merged = LocalAllocation(merged.cast());

    check(
        unsafe {
            SetNamedSecurityInfoW(
                name,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(merged),
                None,
            )
        },
        "SetNamedSecurityInfoW",
    )
    .with_context(|| format!("granting access to {}", path.display()))
}
