use serde_json::{Value, json};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle as ProcessHandle};
use std::{io, mem::size_of};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT},
    System::{
        Diagnostics::{
            Debug::{
                CloseThreadWaitChainSession, GetThreadWaitChain, OpenThreadWaitChainSession,
                WAITCHAIN_NODE_INFO, WCT_MAX_NODE_COUNT, WctThreadType,
            },
            ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
            },
        },
        ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        },
        Threading::{
            GetProcessTimes, GetThreadTimes, OpenProcess, OpenThread, PROCESS_QUERY_INFORMATION,
            PROCESS_SYNCHRONIZE, PROCESS_VM_READ, QueryFullProcessImageNameW,
            THREAD_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
        },
    },
};

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

pub fn handle_live(handle: HANDLE) -> bool {
    unsafe { WaitForSingleObject(handle, 0) == WAIT_TIMEOUT }
}

fn times(handle: HANDLE, thread: bool) -> Option<(u64, u64)> {
    let (mut born, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    let ok = unsafe {
        if thread {
            GetThreadTimes(handle, &mut born, &mut exited, &mut kernel, &mut user)
        } else {
            GetProcessTimes(handle, &mut born, &mut exited, &mut kernel, &mut user)
        }
    };
    (ok != 0).then(|| (ticks(born), ticks(kernel) + ticks(user)))
}

pub struct Process {
    handle: ProcessHandle,
    pub born: u64,
    pid: u32,
}

impl Process {
    pub fn open(pid: u32) -> io::Result<Self> {
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe { ProcessHandle::from_raw_handle(raw) };
        let born = times(raw, false).ok_or_else(io::Error::last_os_error)?.0;
        let mut path = [0u16; 32768];
        let mut len = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(raw, 0, path.as_mut_ptr(), &mut len) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let image = String::from_utf16_lossy(&path[..len as usize]);
        if !image.to_ascii_lowercase().ends_with("\\bun.exe") {
            return Err(io::Error::other("refusing non-Bun target"));
        }
        Ok(Self { handle, born, pid })
    }

    pub fn live(&self) -> bool {
        handle_live(self.handle.as_raw_handle())
    }

    pub fn metrics(&self) -> Value {
        let mut m = PROCESS_MEMORY_COUNTERS_EX {
            cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        let ok = unsafe {
            GetProcessMemoryInfo(
                self.handle.as_raw_handle(),
                (&mut m as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
                size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            )
        };
        if ok == 0 {
            return json!({"win_error":unsafe { GetLastError() }});
        }
        json!({"private_bytes":m.PrivateUsage,"working_set":m.WorkingSetSize,"cpu_100ns":times(self.handle.as_raw_handle(), false).map(|t|t.1)})
    }

    pub fn wait_snapshot(&self) -> Value {
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD | TH32CS_SNAPPROCESS, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return json!({"snapshot_error":unsafe {GetLastError()}});
        }
        let handle = OwnedHandle(raw);
        let mut thread_entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut threads = Vec::new();
        let mut ok = unsafe { Thread32First(handle.0, &mut thread_entry) };
        while ok != 0 {
            if thread_entry.th32OwnerProcessID == self.pid {
                let thread = unsafe {
                    OpenThread(
                        THREAD_QUERY_LIMITED_INFORMATION,
                        0,
                        thread_entry.th32ThreadID,
                    )
                };
                if !thread.is_null() {
                    let owned = OwnedHandle(thread);
                    if let Some((born, cpu)) = times(owned.0, true) {
                        threads.push((born, thread_entry.th32ThreadID, cpu, owned));
                    }
                }
            }
            ok = unsafe { Thread32Next(handle.0, &mut thread_entry) };
        }
        threads.sort_unstable_by_key(|t| (t.0, t.1));
        let mut child_entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut children = Vec::new();
        ok = unsafe { Process32FirstW(handle.0, &mut child_entry) };
        while ok != 0 {
            if child_entry.th32ParentProcessID == self.pid {
                let end = child_entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(child_entry.szExeFile.len());
                let name =
                    String::from_utf16_lossy(&child_entry.szExeFile[..end]).to_ascii_lowercase();
                let kind = match name.as_str() {
                    "icacls.exe" => "acl",
                    "powershell.exe" | "pwsh.exe" | "cmd.exe" => "shell",
                    "bun.exe" => "bun",
                    _ => "other",
                };
                children.push(json!({"pid":child_entry.th32ProcessID,"kind":kind}));
            }
            ok = unsafe { Process32NextW(handle.0, &mut child_entry) };
        }
        let session = unsafe { OpenThreadWaitChainSession(0, None) };
        let mut waits = Vec::new();
        if !session.is_null() {
            // Oldest thread is retained as a hypothesis, never labeled as the JS thread.
            // Names, addresses, object paths, and process command lines are deliberately omitted.
            for (_, tid, cpu, _held_thread) in threads.iter().take(8) {
                let mut count = WCT_MAX_NODE_COUNT;
                let mut nodes = [WAITCHAIN_NODE_INFO::default(); WCT_MAX_NODE_COUNT as usize];
                let mut cycle = 0;
                let result = unsafe {
                    GetThreadWaitChain(
                        session,
                        0,
                        0,
                        *tid,
                        &mut count,
                        nodes.as_mut_ptr(),
                        &mut cycle,
                    )
                };
                if result == 0 {
                    waits.push(
                        json!({"tid":tid,"cpu_100ns":cpu,"win_error":unsafe{GetLastError()}}),
                    );
                } else {
                    let chain: Vec<_> = nodes.iter().take(count.min(WCT_MAX_NODE_COUNT) as usize).map(|n| {
                        if n.ObjectType == WctThreadType {
                            let t = unsafe { n.Anonymous.ThreadObject };
                            json!({"type":n.ObjectType,"status":n.ObjectStatus,"pid":t.ProcessId,"tid":t.ThreadId,"wait_ms":t.WaitTime,"switches":t.ContextSwitches})
                        } else { json!({"type":n.ObjectType,"status":n.ObjectStatus}) }
                    }).collect();
                    waits.push(json!({"tid":tid,"cpu_100ns":cpu,"cycle":cycle!=0,"chain":chain}));
                }
            }
            unsafe {
                CloseThreadWaitChainSession(session);
            }
        }
        json!({"at_ms":super::epoch_ms(),"thread_count":threads.len(),"oldest_tid":threads.first().map(|t|t.1),"waits":waits,"children":children,"wct_available":!session.is_null()})
    }
}
