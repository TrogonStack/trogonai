//! The job object of a lease. It ends every process of the lease when it is
//! closed, caps how many processes and how much memory and CPU time the lease
//! uses, and blocks the payload from the clipboard, global atoms, display
//! settings and handles of windows outside the job.

use std::ffi::c_void;
use std::mem::size_of;

use anyhow::Context;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_JOB_TIME, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS, JOB_OBJECT_UILIMIT_GLOBALATOMS,
    JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
    JOB_OBJECT_UILIMIT_WRITECLIPBOARD, JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
};

use super::constants::{HUNDRED_NANOSECONDS_PER_SECOND, KILLED_EXIT_CODE};
use super::handle::OwnedHandle;
use crate::isolation::resource_limits::ResourceLimits;

pub struct Job(OwnedHandle);

fn limit_information(limits: &ResourceLimits) -> JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    let basic = &mut info.BasicLimitInformation;
    basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    basic.ActiveProcessLimit = limits.processes.count();
    if let Some(cpu) = limits.cpu_time {
        basic.LimitFlags |= JOB_OBJECT_LIMIT_JOB_TIME;
        basic.PerJobUserTimeLimit = i64::from(cpu.seconds()) * HUNDRED_NANOSECONDS_PER_SECOND;
    }
    if let Some(memory) = limits.memory {
        basic.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
        info.JobMemoryLimit = usize::try_from(memory.bytes()).unwrap_or(usize::MAX);
    }
    info
}

fn ui_restrictions() -> JOBOBJECT_BASIC_UI_RESTRICTIONS {
    JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
            | JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_EXITWINDOWS,
    }
}

impl Job {
    pub fn create(limits: &ResourceLimits) -> anyhow::Result<Self> {
        let handle = unsafe { CreateJobObjectW(None, None) }.context("CreateJobObjectW")?;
        let job = Self(OwnedHandle::new(handle));
        let info = limit_information(limits);
        unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast::<c_void>(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .context("SetInformationJobObject(limits)")?;
        let ui = ui_restrictions();
        unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectBasicUIRestrictions,
                std::ptr::from_ref(&ui).cast::<c_void>(),
                size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
            )
        }
        .context("SetInformationJobObject(ui restrictions)")?;
        Ok(job)
    }

    fn raw(&self) -> HANDLE {
        self.0.raw()
    }

    pub fn assign(&self, process: HANDLE) -> anyhow::Result<()> {
        unsafe { AssignProcessToJobObject(self.raw(), process) }.context("AssignProcessToJobObject")
    }

    pub fn terminate(&self) {
        unsafe {
            let _ = TerminateJobObject(self.raw(), KILLED_EXIT_CODE);
        }
    }
}
