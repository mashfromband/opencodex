use serde_json::{Value, json};
use std::{io, mem::size_of, os::windows::io::AsRawHandle, process::Child};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::WaitForSingleObject,
    },
};

pub struct Job(HANDLE);
impl Job {
    pub fn new() -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(core::ptr::null(), core::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(handle);
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
    pub fn attach(&self, child: &Child) -> io::Result<()> {
        if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle().cast()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn active(&self) -> io::Result<u32> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        if unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                core::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(info.ActiveProcesses)
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            TerminateJobObject(self.0, 1);
            CloseHandle(self.0);
        }
    }
}
pub struct OwnedChild(pub Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|exit| exit.is_some()) {
            return;
        }
        let _ = self.0.kill();
        // Never follow a failed kill with an unbounded Child::wait.
        if unsafe { WaitForSingleObject(self.0.as_raw_handle().cast(), 5000) } != WAIT_OBJECT_0 {
            eprintln!("owned_child_reap_deadline");
        }
    }
}
pub fn memory(child: &Child) -> io::Result<Value> {
    let mut info = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    if unsafe {
        K32GetProcessMemoryInfo(
            child.as_raw_handle().cast(),
            (&mut info as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(
        json!({"private_bytes":info.PrivateUsage,"working_set":info.WorkingSetSize,
        "peak_working_set":info.PeakWorkingSetSize,"page_faults":info.PageFaultCount}),
    )
}
